//! One original host per HTTP attempt; no SDK event reducer or new renderer.
use super::wire::{Decoder, Frame};
use crate::proxy::{
    error::ProxyError,
    hyper_client::ProxyResponse,
    qoder_host::{HostConfig, HostError, QoderHost},
};
use futures::StreamExt;
use serde_json::Value;
use std::time::Duration;

fn failed() -> ProxyError {
    // Includes a submitted request whose reply was lost. This is deliberately
    // outside the original pool's retryable Qoder preflight categories.
    ProxyError::ForwardFailed("qoder_http_outcome_unverified".into())
}

// This is the bundled producer's wire contract, not an arbitrary SSE parser.
// Its finish frame is encoded and bounded to one 48 KiB worker frame before
// enqueue; ordinary large text may still be split and is forwarded immediately.
fn is_finish_frame(bytes: &[u8]) -> bool {
    let Some(json) = bytes
        .strip_prefix(b"data: ")
        .and_then(|v| v.strip_suffix(b"\n\n"))
    else {
        return false;
    };
    serde_json::from_slice::<Value>(json)
        .ok()
        .and_then(|v| {
            v.get("choices").and_then(Value::as_array).map(|choices| {
                choices
                    .iter()
                    .any(|choice| choice.get("finish_reason").is_some_and(|v| !v.is_null()))
            })
        })
        .unwrap_or(false)
}

async fn next(
    host: &mut QoderHost,
    deadline: Option<tokio::time::Instant>,
) -> Result<Option<Value>, HostError> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, host.next_event())
            .await
            .map_err(|_| HostError::Interrupted)?,
        None => host.next_event().await,
    }
}

pub(super) async fn forward(
    config: &HostConfig,
    private_request: &Value,
    streaming: bool,
    first_byte_timeout: Duration,
    idle_timeout: Duration,
    non_streaming_timeout: Duration,
) -> Result<ProxyResponse, ProxyError> {
    let operation = async {
        let mut host = QoderHost::start(config, private_request)
            .await
            .map_err(|_| failed())?;
        let mut decoder = Decoder::new();
        let first_deadline = (!first_byte_timeout.is_zero())
            .then(|| tokio::time::Instant::now() + first_byte_timeout);
        let head = next(&mut host, first_deadline).await;
        let (status, headers) = match head {
            Ok(Some(value)) => match decoder.event(&value) {
                Ok(Frame::Head(status, headers)) => (status, headers),
                _ => {
                    let _ = host.close_and_reap().await;
                    return Err(failed());
                }
            },
            _ => {
                let _ = host.close_and_reap().await;
                return Err(failed());
            }
        };
        // Once a task starts, even a valid upstream failure status must not
        // silently rotate accounts or replay a potentially charged operation.
        if !status.is_success() {
            let _ = host.close_and_reap().await;
            return Err(failed());
        }
        let first_result = next(&mut host, first_deadline).await;
        let first = match first_result {
            Ok(Some(value)) => match decoder.event(&value) {
                Ok(Frame::Data(bytes)) => bytes,
                _ => {
                    let _ = host.close_and_reap().await;
                    return Err(failed());
                }
            },
            _ => {
                let _ = host.close_and_reap().await;
                return Err(failed());
            }
        };
        let output = async_stream::stream! {
            // Withhold both Chat success and DONE until clean end + exit 0.
            let mut terminal = None;
            let mut finish = None;
            let mut emitted = false;
            if streaming && is_finish_frame(&first) {
                finish = Some(first);
            } else if streaming && first.as_ref() == b"data: [DONE]\n\n" {
                let _ = host.close_and_reap().await;
                yield Err(std::io::Error::other("qoder_http_finish_missing"));
                return;
            } else {
                emitted = true;
                yield Ok::<_, std::io::Error>(first);
            }
            loop {
                let deadline = if emitted {
                    (!idle_timeout.is_zero()).then(|| tokio::time::Instant::now() + idle_timeout)
                } else {
                    first_deadline
                };
                match next(&mut host, deadline).await {
                    Ok(Some(value)) => match decoder.event(&value) {
                        Ok(Frame::Data(bytes)) => {
                            let done = streaming && bytes.as_ref() == b"data: [DONE]\n\n";
                            if terminal.is_some() || (finish.is_some() && !done) {
                                let _ = host.close_and_reap().await;
                                yield Err(std::io::Error::other("qoder_http_data_after_terminal"));
                                break;
                            }
                            if done {
                                if finish.is_none() {
                                    let _ = host.close_and_reap().await;
                                    yield Err(std::io::Error::other("qoder_http_finish_missing"));
                                    break;
                                }
                                terminal = Some(bytes);
                            } else if streaming && is_finish_frame(&bytes) {
                                finish = Some(bytes);
                            } else {
                                emitted = true;
                                yield Ok(bytes);
                            }
                        },
                        _ => {
                            let _ = host.close_and_reap().await;
                            yield Err(std::io::Error::other("qoder_http_protocol_failed"));
                            break;
                        }
                    },
                    Ok(None) if decoder.finish().is_ok() => {
                        if streaming {
                            if let (Some(final_chunk), Some(done)) = (finish.take(), terminal.take()) {
                                yield Ok(final_chunk);
                                yield Ok(done);
                            } else {
                                yield Err(std::io::Error::other("qoder_http_terminal_missing"));
                            }
                        }
                        break;
                    },
                    _ => {
                        let _ = host.close_and_reap().await;
                        yield Err(std::io::Error::other("qoder_http_outcome_unverified"));
                        break;
                    }
                }
            }
        };
        if streaming {
            Ok(ProxyResponse::streamed(status, headers, output))
        } else {
            let mut output = Box::pin(output);
            let mut bytes = Vec::new();
            while let Some(chunk) = output.next().await {
                bytes.extend_from_slice(&chunk.map_err(|_| failed())?);
            }
            Ok(ProxyResponse::buffered(status, headers, bytes.into()))
        }
    };
    if streaming || non_streaming_timeout.is_zero() {
        operation.await
    } else {
        tokio::time::timeout(non_streaming_timeout, operation)
            .await
            .map_err(|_| failed())?
    }
}
