use std::{str::FromStr, time::Duration};

use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};

pub(crate) mod anthropic;
pub(crate) mod anthropic_passive;
pub(crate) mod openai_responses;
pub(crate) mod openai_responses_passive;

use crate::{
    app_config::AppType,
    relay::model_verification::{
        passive::{
            EvidenceBatch, MAX_RESPONSE_INSPECTION_BYTES,
            MAX_SSE_EVENT_BYTES as PASSIVE_MAX_SSE_EVENT_BYTES,
        },
        types::TargetKey,
    },
};

/// Protocol-neutral passive observer. It only retains bounded parser state and finite facts.
pub(crate) enum VerificationTap {
    Anthropic(anthropic_passive::AnthropicPassiveTap),
    OpenAiResponses(openai_responses_passive::OpenAiResponsesPassiveTap),
}

impl VerificationTap {
    pub(crate) fn new(target: TargetKey, generation: u64) -> Option<Self> {
        match AppType::from_str(target.app_type.as_str()).ok()? {
            AppType::Claude => Some(Self::Anthropic(
                anthropic_passive::AnthropicPassiveTap::new(target, generation),
            )),
            AppType::Codex => Some(Self::OpenAiResponses(
                openai_responses_passive::OpenAiResponsesPassiveTap::new(target, generation),
            )),
            _ => None,
        }
    }

    pub(crate) fn observe_chunk(&mut self, chunk: &[u8]) {
        match self {
            Self::Anthropic(tap) => tap.observe_chunk(chunk),
            Self::OpenAiResponses(tap) => tap.observe_chunk(chunk),
        }
    }

    pub(crate) fn finish(self, completed: bool, observed_at: i64) -> EvidenceBatch {
        match self {
            Self::Anthropic(tap) => tap.finish(completed, observed_at),
            Self::OpenAiResponses(tap) => tap.finish(completed, observed_at),
        }
    }

    pub(crate) fn reduce_non_streaming(
        target: TargetKey,
        generation: u64,
        body: &[u8],
        observed_at: i64,
    ) -> Option<EvidenceBatch> {
        match AppType::from_str(target.app_type.as_str()).ok()? {
            AppType::Claude => Some(
                anthropic_passive::AnthropicPassiveTap::reduce_non_streaming(
                    target,
                    generation,
                    body,
                    observed_at,
                ),
            ),
            AppType::Codex => Some(
                openai_responses_passive::OpenAiResponsesPassiveTap::reduce_non_streaming(
                    target,
                    generation,
                    body,
                    observed_at,
                ),
            ),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        match self {
            Self::Anthropic(tap) => tap.retained_bytes(),
            Self::OpenAiResponses(tap) => tap.retained_bytes(),
        }
    }
}

pub(crate) const MAX_RESPONSE_BYTES: usize = MAX_RESPONSE_INSPECTION_BYTES;
pub(crate) const MAX_SSE_EVENT_BYTES: usize = PASSIVE_MAX_SSE_EVENT_BYTES;
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Finite, body-free reasons that an active protocol probe could not finish.
///
/// This stays inside the protocol boundary until the active-run coordinator maps it to its
/// user-facing run state.  In particular, it never carries a response body, URL, or credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunFailure {
    Authentication,
    RateLimited,
    InsufficientBalance,
    Upstream,
    Network,
    Timeout,
    ModelUnavailable,
    InvalidResponse,
    ResponseTooLarge,
}

pub(crate) async fn send_and_read(request: RequestBuilder) -> Result<Vec<u8>, RunFailure> {
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let response = request.send().await.map_err(map_transport_failure)?;
        let status = response.status();
        let body = read_body(response).await?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(classify_http_failure(status, &body))
        }
    })
    .await
    .map_err(|_| RunFailure::Timeout)?
}

pub(crate) async fn read_body(response: Response) -> Result<Vec<u8>, RunFailure> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_transport_failure)?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(RunFailure::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads an SSE response a single event at a time.  The callback must reduce each event into
/// finite state; this helper discards the event text before accepting the next one.
pub(crate) async fn read_sse(
    response: Response,
    mut on_event: impl FnMut(&str) -> Result<(), RunFailure>,
) -> Result<(), RunFailure> {
    let mut event = Vec::new();
    let mut pending = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_transport_failure)?;
        pending.extend_from_slice(&chunk);

        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = pending.drain(..=newline).collect();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                if !event.is_empty() {
                    on_event(
                        std::str::from_utf8(&event).map_err(|_| RunFailure::InvalidResponse)?,
                    )?;
                    event.clear();
                }
                continue;
            }
            if event.len().saturating_add(line.len()).saturating_add(1) > MAX_SSE_EVENT_BYTES {
                return Err(RunFailure::ResponseTooLarge);
            }
            event.extend_from_slice(&line);
            event.push(b'\n');
        }

        if pending.len().saturating_add(event.len()) > MAX_SSE_EVENT_BYTES {
            return Err(RunFailure::ResponseTooLarge);
        }
    }

    if !pending.is_empty() {
        if event.len().saturating_add(pending.len()) > MAX_SSE_EVENT_BYTES {
            return Err(RunFailure::ResponseTooLarge);
        }
        event.extend_from_slice(&pending);
    }
    if !event.is_empty() {
        on_event(std::str::from_utf8(&event).map_err(|_| RunFailure::InvalidResponse)?)?;
    }
    Ok(())
}

pub(crate) fn classify_http_failure(status: StatusCode, body: &[u8]) -> RunFailure {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => RunFailure::Authentication,
        StatusCode::TOO_MANY_REQUESTS => RunFailure::RateLimited,
        StatusCode::PAYMENT_REQUIRED if indicates_insufficient_balance(body) => {
            RunFailure::InsufficientBalance
        }
        StatusCode::NOT_FOUND => RunFailure::ModelUnavailable,
        status if status.is_server_error() => RunFailure::Upstream,
        _ => RunFailure::InvalidResponse,
    }
}

fn indicates_insufficient_balance(body: &[u8]) -> bool {
    let body = String::from_utf8_lossy(body).to_ascii_lowercase();
    [
        "insufficient balance",
        "insufficient credits",
        "余额不足",
        "额度不足",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

pub(crate) async fn send_sse(
    request: RequestBuilder,
    on_event: impl FnMut(&str) -> Result<(), RunFailure>,
) -> Result<(), RunFailure> {
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let response = request.send().await.map_err(map_transport_failure)?;
        if !response.status().is_success() {
            let status = response.status();
            let body = read_body(response).await?;
            return Err(classify_http_failure(status, &body));
        }
        read_sse(response, on_event).await
    })
    .await
    .map_err(|_| RunFailure::Timeout)?
}

fn map_transport_failure(error: reqwest::Error) -> RunFailure {
    if error.is_timeout() {
        RunFailure::Timeout
    } else {
        RunFailure::Network
    }
}
