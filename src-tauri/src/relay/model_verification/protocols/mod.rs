use std::time::Duration;

use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};

pub(crate) mod anthropic;
pub(crate) mod openai_responses;

pub(crate) const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;
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
    /// An upstream 5xx response. The status is retained for diagnostics, but the response body
    /// deliberately never crosses this protocol boundary.
    Upstream {
        status: u16,
    },
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
        status if status.is_server_error() => RunFailure::Upstream {
            status: status.as_u16(),
        },
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
