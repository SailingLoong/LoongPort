//! One task-owned Node worker, private JSONL pipes, no account discovery or SDK setup.
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use zeroize::Zeroizing;
const MAX_LINE: usize = 1024 * 1024;
const STOP_GRACE: Duration = Duration::from_secs(2);
#[derive(Clone)]
pub struct HostConfig {
    pub node: PathBuf,
    pub worker: PathBuf,
}
#[derive(Debug, PartialEq, Eq)]
pub enum HostError {
    Start,
    Protocol,
    Interrupted,
    Transport,
    Cleanup,
}
/// Contains no auth snapshot or diagnostic rendering of the private request.
pub struct QoderHost {
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    binding: [String; 3],
    partial: Vec<u8>,
    ended: bool,
    cancel_sent: bool,
    reaped_successfully: Option<bool>,
}
impl QoderHost {
    pub async fn start(config: &HostConfig, request: &Value) -> Result<Self, HostError> {
        let mut binding = Vec::new();
        for key in ["accountId", "sessionId", "turnId"] {
            binding.push(
                request["binding"][key]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or(HostError::Protocol)?
                    .to_owned(),
            );
        }
        #[derive(serde::Serialize)]
        struct Start<'a> {
            kind: &'static str,
            request: &'a Value,
        }
        let mut frame = Zeroizing::new(
            serde_json::to_vec(&Start {
                kind: "start",
                request,
            })
            .map_err(|_| HostError::Protocol)?,
        );
        frame.push(b'\n');
        if frame.len() > MAX_LINE {
            return Err(HostError::Protocol);
        }
        // Program and script are trusted backend configuration, never HTTP/IPC input.
        let mut command = Command::new(&config.node);
        command.arg(&config.worker);
        let mut child = command
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| HostError::Start)?;
        let pipes = (child.stdin.take(), child.stdout.take());
        let (input, output) = match pipes {
            (Some(input), Some(output)) => (input, BufReader::new(output)),
            (input, output) => {
                drop(input);
                drop(output);
                return match tokio::time::timeout(STOP_GRACE, child.kill()).await {
                    Ok(Ok(())) => Err(HostError::Start),
                    _ => Err(HostError::Cleanup),
                };
            }
        };
        let mut host = Self {
            child: Some(child),
            input: Some(input),
            output,
            binding: binding.try_into().map_err(|_| HostError::Protocol)?,
            partial: Vec::new(),
            ended: false,
            cancel_sent: false,
            reaped_successfully: None,
        };
        let written =
            tokio::time::timeout(STOP_GRACE, host.input.as_mut().unwrap().write_all(&frame)).await;
        if !matches!(written, Ok(Ok(()))) {
            host.close_and_reap().await?;
            return Err(HostError::Start);
        }
        Ok(host)
    }
    async fn stop(&mut self) -> bool {
        self.ended = true;
        drop(self.input.take());
        // Keep Child in Self across await: timeout/cancellation cannot discard
        // the handle and make a later call report a process was reaped.
        let Some(child) = self.child.as_mut() else {
            return self.reaped_successfully.unwrap_or(false);
        };
        if let Ok(Ok(status)) = tokio::time::timeout(STOP_GRACE, child.wait()).await {
            let success = status.success();
            self.reaped_successfully = Some(success);
            self.child.take();
            return success;
        }
        if matches!(
            tokio::time::timeout(STOP_GRACE, child.kill()).await,
            Ok(Ok(()))
        ) {
            self.reaped_successfully = Some(false);
            self.child.take();
        }
        false
    }
    pub(crate) async fn close_and_reap(&mut self) -> Result<(), HostError> {
        // Ask the same per-request worker to abort before closing its input.
        // A blocked pipe cannot hold cleanup indefinitely; EOF remains the fallback.
        let _ = tokio::time::timeout(STOP_GRACE, self.cancel()).await;
        self.stop().await;
        if self.child.is_none() && self.reaped_successfully.is_some() {
            Ok(())
        } else {
            Err(HostError::Cleanup)
        }
    }
    pub async fn next_event(&mut self) -> Result<Option<Value>, HostError> {
        if self.ended {
            return Ok(None);
        }
        let read = (&mut self.output)
            .take((MAX_LINE - self.partial.len()) as u64)
            .read_until(b'\n', &mut self.partial)
            .await;
        let line = std::mem::take(&mut self.partial);
        let outcome = match read {
            Ok(0) => Err(HostError::Interrupted),
            Ok(_) if line.last() != Some(&b'\n') => Err(HostError::Protocol),
            Ok(_) => match serde_json::from_slice::<Value>(&line) {
                Ok(frame) => match frame["kind"].as_str() {
                    Some("event") => {
                        let event = &frame["event"];
                        if ["accountId", "sessionId", "turnId"]
                            .iter()
                            .zip(&self.binding)
                            .all(|(key, id)| event[*key].as_str() == Some(id.as_str()))
                        {
                            Ok(Some(event.clone()))
                        } else {
                            Err(HostError::Protocol)
                        }
                    }
                    Some("end") => {
                        let valid = frame["cleanupFailed"].is_boolean()
                            && frame["transportFailed"].is_boolean();
                        let error = if !valid {
                            Some(HostError::Protocol)
                        } else if frame["cleanupFailed"] == true {
                            Some(HostError::Cleanup)
                        } else if frame["transportFailed"] == true {
                            Some(HostError::Transport)
                        } else {
                            None
                        };
                        let clean = self.stop().await;
                        return match error {
                            Some(error) => Err(error),
                            None if clean => Ok(None),
                            None => Err(HostError::Interrupted),
                        };
                    }
                    Some("error") => Err(HostError::Transport),
                    _ => Err(HostError::Protocol),
                },
                Err(_) => Err(HostError::Protocol),
            },
            Err(_) => Err(HostError::Interrupted),
        };
        if outcome.is_err() {
            self.stop().await;
        }
        outcome
    }
    /// Use alongside next_event in the caller's select loop. No remote-stop claim.
    pub async fn cancel(&mut self) -> Result<bool, HostError> {
        if self.ended || self.cancel_sent {
            return Ok(false);
        }
        self.cancel_sent = true;
        if self
            .input
            .as_mut()
            .ok_or(HostError::Interrupted)?
            .write_all(b"{\"kind\":\"cancel\"}\n")
            .await
            .is_err()
        {
            self.stop().await;
            return Err(HostError::Interrupted);
        }
        Ok(true)
    }
}
impl Drop for QoderHost {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let input = self.input.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let graceful = async {
                    if let Some(mut input) = input {
                        let _ = input.write_all(b"{\"kind\":\"cancel\"}\n").await;
                        drop(input);
                    }
                    let _ = child.wait().await;
                };
                if tokio::time::timeout(STOP_GRACE, graceful).await.is_err()
                    && !matches!(
                        tokio::time::timeout(STOP_GRACE, child.kill()).await,
                        Ok(Ok(()))
                    )
                {
                    log::warn!("Qoder worker cleanup could not confirm child reaping");
                }
            });
        }
        // Without a live runtime, Child's kill_on_drop remains the local fallback.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    fn config(name: &str) -> HostConfig {
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
                .map(PathBuf::from)
                .unwrap_or_else(|| "node".into()),
            worker: root.join("tests/qoder-runtime/fixtures").join(name),
        }
    }
    fn request() -> Value {
        json!({"binding":{"accountId":"account","sessionId":"session","turnId":"turn"},"prompt":"fixture","options":{"auth":{"type":"accessToken","accessToken":"synthetic:pat"}}})
    }
    async fn next(host: &mut QoderHost) -> Result<Option<Value>, HostError> {
        // Includes starting an external Node process. Windows CI can cold-start
        // several fixtures together while the full Rust suite is still busy.
        // Keep a finite test deadline without imposing a 5s startup contract.
        tokio::time::timeout(Duration::from_secs(30), host.next_event())
            .await
            .expect("worker timeout")
    }
    #[tokio::test]
    async fn actual_child_entry_forwards_text_terminal_then_eof() {
        let mut h = QoderHost::start(&config("success.mjs"), &request())
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(e) = next(&mut h).await.unwrap() {
            events.push(e);
        }
        assert!(events.iter().any(|e| e["text"] == "fixture-reply"));
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "terminal" && e["status"] == "success")
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn cancel_command_is_idempotent_and_reaches_query() {
        let mut h = QoderHost::start(&config("cancel.mjs"), &request())
            .await
            .unwrap();
        assert!(next(&mut h).await.unwrap().is_some());
        assert!(h.cancel().await.unwrap());
        assert!(!h.cancel().await.unwrap());
        let mut cancelled = false;
        while let Some(e) = next(&mut h).await.unwrap() {
            cancelled |= e["status"] == "cancelled";
        }
        assert!(cancelled);
    }
    #[tokio::test]
    async fn explicit_close_sends_cancel_and_confirms_child_reaping() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("explicit-close");
        let mut r = request();
        r["options"]["syntheticMarker"] = json!(marker);
        let mut h = QoderHost::start(&config("cancel.mjs"), &r).await.unwrap();
        assert!(next(&mut h).await.unwrap().is_some());
        h.close_and_reap().await.unwrap();
        assert!(h.cancel_sent);
        assert!(h.child.is_none());
        assert!(h.reaped_successfully.is_some());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "closed");
        h.close_and_reap().await.unwrap();
        assert!(!h.cancel().await.unwrap());
    }
    #[tokio::test]
    async fn source_failure_reaches_host_without_private_error() {
        let mut h = QoderHost::start(&config("failure.mjs"), &request())
            .await
            .unwrap();
        let e = next(&mut h).await.unwrap().unwrap();
        assert_eq!(e["status"], "error");
        assert_eq!(next(&mut h).await, Err(HostError::Transport));
    }
    #[tokio::test]
    async fn abrupt_child_eof_never_promotes_partial_to_success() {
        let mut h = QoderHost::start(&config("interrupted.mjs"), &request())
            .await
            .unwrap();
        assert_eq!(next(&mut h).await.unwrap().unwrap()["text"], "partial");
        assert_eq!(next(&mut h).await, Err(HostError::Interrupted));
    }
    #[tokio::test]
    async fn foreign_binding_is_rejected_at_rust_boundary() {
        let mut h = QoderHost::start(&config("foreign.mjs"), &request())
            .await
            .unwrap();
        assert_eq!(next(&mut h).await, Err(HostError::Protocol));
    }
    #[tokio::test]
    async fn cancelled_read_preserves_partial_frame() {
        let mut h = QoderHost::start(&config("fragmented.mjs"), &request())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(80), h.next_event())
                .await
                .is_err()
        );
        assert_eq!(next(&mut h).await.unwrap().unwrap()["text"], "fragmented");
        assert_eq!(next(&mut h).await.unwrap(), None);
    }
    #[tokio::test]
    async fn drop_sends_cancel_before_bounded_child_reaping() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("closed");
        let mut r = request();
        r["options"]["syntheticMarker"] = json!(marker);
        let mut h = QoderHost::start(&config("cancel.mjs"), &r).await.unwrap();
        next(&mut h).await.unwrap();
        drop(h);
        tokio::time::timeout(Duration::from_secs(4), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Query not closed on drop");
    }
}
