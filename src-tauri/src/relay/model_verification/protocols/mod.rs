use std::time::Duration;

use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};

pub(crate) mod anthropic;
pub(crate) mod openai_responses;

pub(crate) const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETRIES: usize = 2;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

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
    for attempt in 0..=MAX_RETRIES {
        let request = request.try_clone().ok_or(RunFailure::InvalidResponse)?;
        let result = tokio::time::timeout(PROBE_TIMEOUT, async {
            let response = request.send().await.map_err(map_transport_failure)?;
            let status = response.status();
            if status.is_server_error() {
                return Err(classify_http_failure(status, &[]));
            }
            let body = read_body(response).await?;
            if status.is_success() {
                Ok(body)
            } else {
                Err(classify_http_failure(status, &body))
            }
        })
        .await
        .map_err(|_| RunFailure::Timeout)?;
        match result {
            Err(failure) if attempt < MAX_RETRIES && is_retryable_failure(failure) => {
                tokio::time::sleep(RETRY_BASE_DELAY * (1 << attempt)).await;
            }
            result => return result,
        }
    }
    unreachable!("retry loop always returns")
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

pub(crate) async fn send_sse<State>(
    request: RequestBuilder,
    mut new_state: impl FnMut() -> State,
    mut on_event: impl FnMut(&mut State, &str) -> Result<(), RunFailure>,
) -> Result<State, RunFailure> {
    for attempt in 0..=MAX_RETRIES {
        let request = request.try_clone().ok_or(RunFailure::InvalidResponse)?;
        let mut state = new_state();
        let result = tokio::time::timeout(PROBE_TIMEOUT, async {
            let response = request.send().await.map_err(map_transport_failure)?;
            let status = response.status();
            if status.is_server_error() {
                return Err(classify_http_failure(status, &[]));
            }
            if !status.is_success() {
                let body = read_body(response).await?;
                return Err(classify_http_failure(status, &body));
            }
            read_sse(response, |event| on_event(&mut state, event)).await?;
            Ok(state)
        })
        .await
        .map_err(|_| RunFailure::Timeout)?;
        match result {
            Err(failure) if attempt < MAX_RETRIES && is_retryable_failure(failure) => {
                tokio::time::sleep(RETRY_BASE_DELAY * (1 << attempt)).await;
            }
            result => return result,
        }
    }
    unreachable!("retry loop always returns")
}

fn map_transport_failure(error: reqwest::Error) -> RunFailure {
    if error.is_timeout() {
        RunFailure::Timeout
    } else if error.is_connect() || error.is_request() || error.is_body() {
        RunFailure::Network
    } else {
        RunFailure::InvalidResponse
    }
}

fn is_retryable_failure(failure: RunFailure) -> bool {
    matches!(
        failure,
        RunFailure::Network
            | RunFailure::Timeout
            | RunFailure::Upstream {
                status: 500 | 502 | 503 | 504
            }
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        body::Body,
        http::{HeaderValue, StatusCode},
        response::IntoResponse,
        routing::post,
        Router,
    };
    use bytes::Bytes;

    use super::{send_and_read, send_sse, RunFailure, MAX_RESPONSE_BYTES};

    #[tokio::test]
    async fn send_and_read_retries_transient_upstream_failure() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/probe",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            (StatusCode::BAD_GATEWAY, "temporary")
                        } else {
                            (StatusCode::OK, "ready")
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let body = send_and_read(
            reqwest::Client::new()
                .post(format!("http://{address}/probe"))
                .body("{}"),
        )
        .await
        .unwrap();

        assert_eq!(body, b"ready");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn send_and_read_retries_retryable_status_before_reading_oversized_body() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/probe",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            (StatusCode::BAD_GATEWAY, vec![b'x'; MAX_RESPONSE_BYTES + 1])
                        } else {
                            (StatusCode::OK, b"ready".to_vec())
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let body = send_and_read(
            reqwest::Client::new()
                .post(format!("http://{address}/probe"))
                .body("{}"),
        )
        .await
        .unwrap();

        assert_eq!(body, b"ready");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn send_and_read_does_not_retry_redirect_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/probe",
            post({
                let attempts = attempts.clone();
                move || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    async { (StatusCode::FOUND, [("location", "/probe")]) }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                attempt.error("redirect disabled")
            }))
            .build()
            .unwrap();

        let failure = send_and_read(client.post(format!("http://{address}/probe")).body("{}"))
            .await
            .unwrap_err();

        assert_eq!(failure, RunFailure::InvalidResponse);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn send_sse_retries_transient_upstream_failure_with_fresh_state() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/probe",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            let stream = async_stream::stream! {
                                yield Ok::<_, std::io::Error>(Bytes::from_static(b"data: stale\n\n"));
                                yield Err(std::io::Error::other("disconnected"));
                            };
                            let mut response = Body::from_stream(stream).into_response();
                            response.headers_mut().insert(
                                "content-type",
                                HeaderValue::from_static("text/event-stream"),
                            );
                            response
                        } else {
                            (
                                StatusCode::OK,
                                [("content-type", "text/event-stream")],
                                "data: ready\n\n",
                            )
                                .into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let events = send_sse(
            reqwest::Client::new()
                .post(format!("http://{address}/probe"))
                .body("{}"),
            Vec::new,
            |events, event| {
                events.push(event.to_string());
                Ok(())
            },
        )
        .await
        .unwrap();

        assert_eq!(events, ["data: ready\n"]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
