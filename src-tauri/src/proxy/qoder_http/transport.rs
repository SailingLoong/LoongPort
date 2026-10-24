//! One original host per HTTP attempt; no SDK event reducer or new renderer.
use super::wire::{Decoder, Frame};
use crate::proxy::{
    error::ProxyError,
    hyper_client::ProxyResponse,
    qoder_host::{HostConfig, HostError, QoderHost},
};
use futures::StreamExt;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

pub(super) type DeviceRejection = Arc<dyn Fn() -> Result<(), ProxyError> + Send + Sync>;
const REAUTHORIZE: &str = "qoder.http_auth.reauthorization_required";

fn failed() -> ProxyError {
    // Includes a submitted request whose reply was lost. This is deliberately
    // outside the original pool's retryable Qoder preflight categories.
    ProxyError::ForwardFailed("qoder_http_outcome_unverified".into())
}

// This is a fixed producer error, never arbitrary upstream error text. Local
// expiry uses a different code. Only exact device-source refusals may retire
// the original owner, and only after the host confirms clean end and exit 0.
fn device_rejection(bytes: &[u8], streaming: bool) -> bool {
    let bytes = if streaming {
        let Some(bytes) = bytes
            .strip_prefix(b"data: ")
            .and_then(|v| v.strip_suffix(b"\n\n"))
        else {
            return false;
        };
        bytes
    } else {
        bytes
    };
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    value.as_object().is_some_and(|o| o.len() == 1)
        && value["error"]["message"] == "QODER_DEVICE_REAUTHORIZATION_REQUIRED"
        && (value["error"]["code"] == "QODER_DEVICE_REAUTHORIZATION_REQUIRED"
            || matches!(value["error"]["code"].as_u64(), Some(401 | 403)))
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
    on_device_rejected: Option<DeviceRejection>,
) -> Result<ProxyResponse, ProxyError> {
    let device = private_request["snapshot"]["credentialKind"] == "china_qoder_device";
    let operation = async move {
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
            if device && matches!(status.as_u16(), 401 | 403) {
                let rejected = match next(&mut host, first_deadline).await {
                    Ok(Some(value)) => {
                        matches!(decoder.event(&value), Ok(Frame::Data(bytes)) if device_rejection(&bytes, false))
                    }
                    _ => false,
                };
                if rejected
                    && matches!(next(&mut host, first_deadline).await, Ok(None))
                    && decoder.finish().is_ok()
                {
                    if let Some(reject) = &on_device_rejected {
                        reject()?;
                    }
                    return Err(ProxyError::ForwardFailed(REAUTHORIZE.into()));
                }
            }
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
            let mut rejected = None;
            if device && streaming && device_rejection(&first, true) {
                rejected = Some(first);
            } else if streaming && is_finish_frame(&first) {
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
                            if rejected.is_some() || terminal.is_some() || (finish.is_some() && !done) {
                                let _ = host.close_and_reap().await;
                                yield Err(std::io::Error::other("qoder_http_data_after_terminal"));
                                break;
                            }
                            if device && streaming && device_rejection(&bytes, true) {
                                rejected = Some(bytes);
                            } else if done {
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
                        if let Some(bytes) = rejected.take() {
                            if let Some(reject) = &on_device_rejected {
                                if reject().is_err() {
                                    yield Err(std::io::Error::other("qoder.http_auth.authorization_changed"));
                                    break;
                                }
                            }
                            yield Ok(bytes);
                            yield Err(std::io::Error::other(REAUTHORIZE));
                            break;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    // These are protocol/classification tests, not a 5s process-performance SLA.
    // Match the existing qoder_host external-process test budget; production
    // deadlines and the 2s exit/kill grace remain untouched.
    const PROCESS_INTEGRATION_TIMEOUT: Duration = Duration::from_secs(30);

    fn host() -> HostConfig {
        let root = std::env::var_os("QODER_HOST_PRODUCT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .to_owned()
            });
        HostConfig {
            node: std::env::var_os("QODER_HOST_NODE")
                .unwrap_or_else(|| "node".into())
                .into(),
            worker: root.join("tests/qoder-runtime/fixtures/http-auth-rejected.mjs"),
        }
    }
    fn request(mode: &str, kind: &str) -> Value {
        json!({"binding":{"accountId":"synthetic-account","sessionId":"synthetic-session","turnId":"synthetic-turn"},"snapshot":{"credentialKind":kind},"chat":{"model":mode}})
    }
    async fn failure(mode: &str, kind: &str) -> String {
        match forward(
            &host(),
            &request(mode, kind),
            false,
            PROCESS_INTEGRATION_TIMEOUT,
            PROCESS_INTEGRATION_TIMEOUT,
            PROCESS_INTEGRATION_TIMEOUT,
            None,
        )
        .await
        {
            Err(ProxyError::ForwardFailed(code)) => code,
            _ => panic!("a rejected request must never succeed"),
        }
    }
    #[tokio::test]
    async fn clean_device_auth_rejection_has_explicit_nonretryable_reauthorization_error() {
        assert_eq!(
            failure("rejected", "china_qoder_device").await,
            "qoder.http_auth.reauthorization_required"
        );
    }
    #[tokio::test]
    async fn local_expiry_pat_and_unverified_workers_cannot_claim_device_auth_rejection() {
        for (mode, kind) in [
            ("expired", "china_qoder_device"),
            ("rejected", "china_qoder_pat"),
            ("rejected", "international_qoder_pat"),
            ("broken", "china_qoder_device"),
            ("nonzero", "china_qoder_device"),
            ("extra", "china_qoder_device"),
        ] {
            assert_eq!(
                failure(mode, kind).await,
                "qoder_http_outcome_unverified",
                "mode={mode} kind={kind}"
            );
        }
    }
    #[tokio::test]
    async fn rejection_callback_waits_for_verified_worker_end_including_streaming() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (mode, expected) in [
            ("rejected", 1),
            ("stream", 1),
            ("expired", 0),
            ("broken", 0),
            ("nonzero", 0),
            ("extra", 0),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let handler: DeviceRejection = Arc::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            let response = forward(
                &host(),
                &request(mode, "china_qoder_device"),
                mode == "stream",
                PROCESS_INTEGRATION_TIMEOUT,
                PROCESS_INTEGRATION_TIMEOUT,
                PROCESS_INTEGRATION_TIMEOUT,
                Some(handler),
            )
            .await;
            if mode == "stream" {
                let mut stream = match response {
                    Ok(crate::proxy::hyper_client::ProxyResponse::Streamed { stream, .. }) => {
                        stream
                    }
                    Err(error) => panic!("mode={mode}: stream setup failed: {error}"),
                    _ => panic!("mode={mode}: expected streaming response"),
                };
                assert_eq!(calls.load(Ordering::SeqCst), 0, "not before clean end");
                let mut bytes = Vec::new();
                let mut failed = false;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => bytes.extend_from_slice(&chunk),
                        Err(error) => {
                            assert_eq!(error.to_string(), REAUTHORIZE);
                            failed = true;
                        }
                    }
                }
                assert!(failed);
                assert!(!String::from_utf8_lossy(&bytes).contains("[DONE]"));
            } else {
                let expected_error = if expected == 1 {
                    REAUTHORIZE
                } else {
                    "qoder_http_outcome_unverified"
                };
                match response {
                    Err(ProxyError::ForwardFailed(code)) => {
                        assert_eq!(code, expected_error, "mode={mode}: forward result");
                    }
                    Err(error) => panic!("mode={mode}: unexpected forward error: {error}"),
                    _ => panic!("mode={mode}: rejected request unexpectedly succeeded"),
                }
            }
            assert_eq!(calls.load(Ordering::SeqCst), expected, "mode={mode}");
        }
    }

    #[tokio::test]
    async fn production_cn_worker_classification_reaches_original_transport_without_false_retirement(
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (mode, streaming, retired) in [
            ("ok", false, 0),
            ("ok", true, 0),
            ("auth", false, 1),
            ("queue", false, 0),
            ("quota", false, 0),
            ("request", false, 0),
            ("oversize", false, 0),
            ("late-auth", true, 1),
            ("late-quota", true, 0),
        ] {
            let mut config = host();
            config.worker.set_file_name("http-cn-device-contract.mjs");
            let mut input = request(mode, "china_qoder_device");
            input["snapshot"] = json!({"access":"SYNTHETIC_CN_OWNER_ACCESS","uid":"SYNTHETIC_CN_OWNER_UID","machineId":"synthetic-machine","expiry":4102444800000_i64,"region":"cn","credentialKind":"china_qoder_device"});
            input["chat"] = json!({"model":mode,"stream":streaming,"messages":[{"role":"user","content":"synthetic"}]});
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let handler: DeviceRejection = Arc::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            let trace = tempfile::tempdir().unwrap();
            let trace_path = trace.path().join("worker.trace");
            input["testLifecyclePath"] = json!(trace_path);
            let started = std::time::Instant::now();
            let context = || {
                format!(
                    "mode={mode} streaming={streaming} elapsed={:?} fixture_stages={}",
                    started.elapsed(),
                    std::fs::read_to_string(&trace_path).unwrap_or_else(|_| "not-observed".into())
                )
            };
            let result = forward(
                &config,
                &input,
                streaming,
                PROCESS_INTEGRATION_TIMEOUT,
                PROCESS_INTEGRATION_TIMEOUT,
                PROCESS_INTEGRATION_TIMEOUT,
                Some(handler),
            )
            .await;
            match result {
                Ok(crate::proxy::hyper_client::ProxyResponse::Streamed { mut stream, .. }) => {
                    let mut bytes = Vec::new();
                    let mut failed = false;
                    let mut failure_code = None;
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(chunk) => bytes.extend_from_slice(&chunk),
                            Err(error) => {
                                failed = true;
                                failure_code = Some(error.to_string());
                            }
                        }
                    }
                    assert_eq!(
                        failed,
                        mode != "ok",
                        "{} stream_code={failure_code:?}",
                        context()
                    );
                    assert_eq!(
                        String::from_utf8_lossy(&bytes).contains("[DONE]"),
                        mode == "ok",
                        "{} stream_code={failure_code:?}",
                        context()
                    );
                }
                Ok(crate::proxy::hyper_client::ProxyResponse::Buffered { body, .. }) => {
                    assert_eq!(mode, "ok", "{}", context());
                    assert!(
                        String::from_utf8_lossy(&body).contains("synthetic CN success"),
                        "{}",
                        context()
                    );
                }
                Err(ProxyError::ForwardFailed(code)) => {
                    assert_ne!(mode, "ok", "{} forward_code={code}", context());
                    assert_eq!(
                        code == REAUTHORIZE,
                        retired == 1,
                        "{} forward_code={code}",
                        context()
                    );
                }
                _ => panic!("unexpected response: {}", context()),
            }
            assert_eq!(calls.load(Ordering::SeqCst), retired, "{}", context());
        }
    }

    async fn wait_for_fixture_marker(path: &std::path::Path) {
        // Called with the real clock. A ready marker proves Node has consumed
        // the request; a timeout here must not masquerade as a deadline pass.
        tokio::time::timeout(PROCESS_INTEGRATION_TIMEOUT, async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("synthetic worker lifecycle marker missing");
    }

    fn gated_request(mode: &str, prefix: &std::path::Path) -> (HostConfig, Value) {
        let mut config = host();
        config.worker.set_file_name("http-deadline-gated.mjs");
        let mut input = request(mode, "china_qoder_device");
        input["testLifecyclePath"] = json!(prefix);
        (config, input)
    }

    async fn verify_fixture_cancel_and_exit_callback(dir: &std::path::Path) {
        // The exit marker proves the fixture entered its exit callback, not
        // that a parent wait/reap completed. Host reaping tests remain separate.
        wait_for_fixture_marker(&dir.join("gate.cancelled")).await;
        wait_for_fixture_marker(&dir.join("gate.exited")).await;
        assert_eq!(
            std::fs::read_to_string(dir.join("gate.exited")).unwrap(),
            "0"
        );
    }

    #[tokio::test]
    async fn total_deadline_rejects_ready_partial_body_without_retiring_device() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (config, input) = gated_request("nonstream", &dir.path().join("gate"));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let handler: DeviceRejection = Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let task = tokio::spawn(async move {
            forward(
                &config,
                &input,
                false,
                Duration::ZERO,
                Duration::ZERO,
                PROCESS_INTEGRATION_TIMEOUT,
                Some(handler),
            )
            .await
        });
        wait_for_fixture_marker(&dir.path().join("gate.ready")).await;
        assert!(!task.is_finished(), "must still await verified end");
        // The outer total deadline is armed before spawning the OS child.
        // Only advance after readiness; resume before real cancellation/exit.
        tokio::time::pause();
        tokio::time::advance(PROCESS_INTEGRATION_TIMEOUT).await;
        tokio::time::resume();
        let result = tokio::time::timeout(PROCESS_INTEGRATION_TIMEOUT, task)
            .await
            .expect("total deadline did not complete")
            .unwrap();
        assert!(
            matches!(result, Err(ProxyError::ForwardFailed(code)) if code == "qoder_http_outcome_unverified")
        );
        verify_fixture_cancel_and_exit_callback(dir.path()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn first_verification_and_idle_deadlines_never_promote_unverified_streams() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for mode in ["rejection", "idle"] {
            let dir = tempfile::tempdir().unwrap();
            let (config, input) = gated_request(mode, &dir.path().join("gate"));
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let handler: DeviceRejection = Arc::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            let first_timeout = if mode == "rejection" {
                PROCESS_INTEGRATION_TIMEOUT
            } else {
                Duration::ZERO
            };
            let idle_timeout = if mode == "idle" {
                PROCESS_INTEGRATION_TIMEOUT
            } else {
                Duration::ZERO
            };
            let response = tokio::time::timeout(
                PROCESS_INTEGRATION_TIMEOUT,
                forward(
                    &config,
                    &input,
                    true,
                    first_timeout,
                    idle_timeout,
                    Duration::ZERO,
                    Some(handler),
                ),
            )
            .await
            .expect("streaming fixture setup timed out")
            .unwrap();
            // forward has already parsed head and first body. Thus the first
            // timer (when enabled) is armed, independently of OS scheduling.
            // Each mode enables only its own deadline, so using the wrong
            // timer cannot satisfy this regression.
            let crate::proxy::hyper_client::ProxyResponse::Streamed { mut stream, .. } = response
            else {
                panic!("expected streaming response");
            };
            if mode == "idle" {
                let first = stream.next().await.unwrap().unwrap();
                assert!(String::from_utf8_lossy(&first).contains("synthetic prefix"));
                assert!(!String::from_utf8_lossy(&first).contains("[DONE]"));
            }
            wait_for_fixture_marker(&dir.path().join("gate.ready")).await;
            {
                let next = stream.next();
                tokio::pin!(next);
                // For rejection, this processes the exact auth body but waits
                // for clean end under first_deadline. For idle, it arms the
                // post-prefix idle timer. Neither can yield success or retire.
                assert!(futures::poll!(&mut next).is_pending());
                assert_eq!(calls.load(Ordering::SeqCst), 0);
                tokio::time::pause();
                tokio::time::advance(PROCESS_INTEGRATION_TIMEOUT).await;
                tokio::time::resume();
                let error = tokio::time::timeout(PROCESS_INTEGRATION_TIMEOUT, next)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err();
                assert_eq!(error.to_string(), "qoder_http_outcome_unverified");
            }
            assert!(stream.next().await.is_none());
            verify_fixture_cancel_and_exit_callback(dir.path()).await;
            assert_eq!(calls.load(Ordering::SeqCst), 0, "mode={mode}");
        }
    }

    #[tokio::test]
    async fn cancelling_ready_transport_drops_worker_without_retiring_device() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (config, input) = gated_request("first", &dir.path().join("gate"));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let handler: DeviceRejection = Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let task = tokio::spawn(async move {
            forward(
                &config,
                &input,
                false,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Some(handler),
            )
            .await
        });
        wait_for_fixture_marker(&dir.path().join("gate.ready")).await;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        verify_fixture_cancel_and_exit_callback(dir.path()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
