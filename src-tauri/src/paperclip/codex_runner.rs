//! Codex app-server protocol adapter. It owns no task persistence or workspace allocation.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) struct RunnerResult {
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub process_stopped: bool,
}

struct Session {
    child: Child,
    input: Option<mpsc::SyncSender<(Value, mpsc::Sender<Result<(), String>>)>>,
    writer: Option<thread::JoinHandle<()>>,
    messages: Receiver<Result<Value, String>>,
    pending: Vec<Value>,
    next_id: u64,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    cleanup_attempted: bool,
}
impl Session {
    fn send(&mut self, value: Value) -> Result<(), String> {
        let (acknowledge, written) = mpsc::channel();
        self.input
            .as_ref()
            .ok_or("agents.runtime_closed")?
            .try_send((value, acknowledge))
            .map_err(|_| "agents.runtime_write_failed")?;
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err("agents.cancelled".into());
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or("agents.runtime_timeout")?;
            match written.recv_timeout(remaining.min(Duration::from_millis(25))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("agents.runtime_write_failed".into())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn receive(&mut self) -> Result<Value, String> {
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err("agents.cancelled".into());
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or("agents.runtime_timeout")?;
            match self
                .messages
                .recv_timeout(remaining.min(Duration::from_millis(25)))
            {
                Ok(value) => return value,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("agents.runtime_exited".into())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn server_request(&mut self, value: &Value) -> Result<bool, String> {
        if value.get("id").is_none() || value.get("method").is_none() {
            return Ok(false);
        }
        // Unattended runs cannot grant new permissions or answer user questions.
        self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"Interactive requests are unsupported"}}))?;
        Err("agents.runtime_requires_interaction".into())
    }
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.cancel.load(Ordering::Acquire) {
            return Err("agents.cancelled".into());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        loop {
            let value = self.receive()?;
            self.server_request(&value)?;
            if value.get("id").is_some() {
                if value["id"] != id {
                    return Err("agents.runtime_uncorrelated_response".into());
                }
                if value.get("error").is_some() {
                    return Err("agents.runtime_request_failed".into());
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "agents.runtime_invalid_response".into());
            }
            if self.pending.len() >= 128 {
                return Err("agents.runtime_notification_overflow".into());
            }
            self.pending.push(value);
        }
    }
    fn initialize(&mut self) -> Result<(), String> {
        self.request("initialize", json!({"clientInfo":{"name":"loongport","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}}))?;
        self.send(json!({"method":"initialized"}))
    }
    fn verify_isolation(&mut self, cwd: &Path) -> Result<(), String> {
        let response = self.request("config/read", json!({"cwd":cwd,"includeLayers":false}))?;
        let config = response["config"]
            .as_object()
            .ok_or("agents.runtime_unsafe_configuration")?;
        for key in ["mcp_servers", "plugins", "hooks"] {
            if config.get(key).is_some_and(|value| {
                !value.is_null() && !value.as_object().is_some_and(|map| map.is_empty())
            }) {
                return Err("agents.runtime_unsafe_configuration".into());
            }
        }
        if config.get("notify").is_some_and(|value| {
            !value.is_null() && !value.as_array().is_some_and(|items| items.is_empty())
        }) {
            return Err("agents.runtime_unsafe_configuration".into());
        }
        let policy = config
            .get("shell_environment_policy")
            .ok_or("agents.runtime_unsafe_configuration")?;
        if policy["inherit"] != "none"
            || policy["set"].as_object().is_none_or(|set| {
                set.keys()
                    .any(|key| !matches!(key.as_str(), "PATH" | "HOME"))
            })
        {
            return Err("agents.runtime_unsafe_configuration".into());
        }
        if config.get("model_provider").and_then(Value::as_str) != Some("loongport-task") {
            return Err("agents.runtime_unsafe_configuration".into());
        }
        Ok(())
    }
    fn execute(
        &mut self,
        cwd: &Path,
        input: &str,
        model: Option<&str>,
    ) -> Result<(String, Option<String>), String> {
        self.initialize()?;
        self.verify_isolation(cwd)?;
        let mut params = json!({"cwd":cwd,"ephemeral":true,"approvalPolicy":"never","sandbox":"workspace-write"});
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        let started = self.request("thread/start", params)?;
        if started["thread"]["ephemeral"] != true {
            return Err("agents.runtime_not_ephemeral".into());
        }
        let thread_id = started["thread"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("agents.runtime_invalid_thread")?
            .to_owned();
        let turn = self.request("turn/start", json!({"threadId":thread_id,"input":[{"type":"text","text":input}],"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[cwd],"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true}}))?;
        let turn_id = turn["turn"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("agents.runtime_invalid_turn")?
            .to_owned();
        let outcome = self.wait_turn(&thread_id, &turn_id);
        if self.cancel.load(Ordering::Acquire) {
            // Best-effort protocol cancellation precedes mandatory process cleanup.
            let _ = self.send(json!({"id":self.next_id,"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}}));
        }
        outcome
    }
    fn wait_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(String, Option<String>), String> {
        let mut pending = std::mem::take(&mut self.pending).into_iter();
        let mut output = None;
        loop {
            let value = match pending.next() {
                Some(value) => value,
                None => self.receive()?,
            };
            self.server_request(&value)?;
            let params = &value["params"];
            if params["threadId"] != thread_id {
                continue;
            }
            if value["method"] == "item/completed" && params["turnId"] == turn_id {
                if let Some(text) = answer(&params["item"]) {
                    output = Some(text);
                }
            }
            if value["method"] != "turn/completed" || params["turn"]["id"] != turn_id {
                continue;
            }
            let turn = &params["turn"];
            for item in turn["items"]
                .as_array()
                .ok_or("agents.runtime_invalid_turn")?
            {
                if let Some(text) = answer(item) {
                    output = Some(text);
                }
            }
            return match turn["status"].as_str() {
                Some("completed") => Ok(("completed".into(), output)),
                Some("interrupted") => Ok(("cancelled".into(), output)),
                Some("failed") => Err("agents.code_turn_failed".into()),
                _ => Err("agents.runtime_invalid_turn_status".into()),
            };
        }
    }
    fn stop(&mut self) -> bool {
        self.cleanup_attempted = true;
        self.input.take();
        let signalled = crate::process::terminate_child_tree(&mut self.child);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    while self
                        .writer
                        .as_ref()
                        .is_some_and(|writer| !writer.is_finished())
                        && Instant::now() < deadline
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    let writer_stopped = self
                        .writer
                        .as_ref()
                        .is_none_or(|writer| writer.is_finished());
                    if writer_stopped {
                        if let Some(writer) = self.writer.take() {
                            let _ = writer.join();
                        }
                    }
                    return signalled && writer_stopped;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => return false,
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.cleanup_attempted {
            let _ = self.stop();
        }
    }
}
fn answer(item: &Value) -> Option<String> {
    if item["type"] != "agentMessage"
        || item["phase"]
            .as_str()
            .is_some_and(|phase| phase != "final_answer")
    {
        return None;
    }
    item["text"]
        .as_str()
        .filter(|text| text.len() <= 1024 * 1024)
        .map(str::to_owned)
}

fn spawn(
    mut command: Command,
    cwd: &Path,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Session, String> {
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::process::isolate_child_process_group(&mut command);
    let mut child = command.spawn().map_err(|_| "agents.runtime_spawn_failed")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("agents.runtime_stdout_unavailable")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("agents.runtime_stdin_unavailable")?;
    let (input, writes) = mpsc::sync_channel::<(Value, mpsc::Sender<Result<(), String>>)>(1);
    let writer = thread::spawn(move || {
        while let Ok((value, acknowledge)) = writes.recv() {
            let result = serde_json::to_writer(&mut stdin, &value)
                .map_err(|_| "agents.runtime_write_failed".to_string())
                .and_then(|_| {
                    stdin
                        .write_all(b"\n")
                        .and_then(|_| stdin.flush())
                        .map_err(|_| "agents.runtime_write_failed".to_string())
                });
            let failed = result.is_err();
            let _ = acknowledge.send(result);
            if failed {
                return;
            }
        }
    });
    let (send, messages) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut line);
            let value = match read {
                Ok(0) => return,
                Ok(_) if line.len() <= 1024 * 1024 => serde_json::from_slice(&line)
                    .map_err(|_| "agents.runtime_invalid_json".to_string()),
                _ => Err("agents.runtime_frame_too_large".into()),
            };
            let failed = value.is_err();
            if send.send(value).is_err() || failed {
                return;
            }
        }
    });
    Ok(Session {
        child,
        input: Some(input),
        writer: Some(writer),
        messages,
        pending: Vec::new(),
        next_id: 1,
        cancel: cancel.clone(),
        deadline: Instant::now() + timeout,
        cleanup_attempted: false,
    })
}
pub(super) fn run(
    command: Command,
    cwd: &Path,
    input: &str,
    model: Option<&str>,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<RunnerResult, String> {
    if input.trim().is_empty()
        || input.len() > 65536
        || !cwd.is_absolute()
        || cancel.load(Ordering::Acquire)
    {
        return Err("agents.invalid_code_task".into());
    }
    let mut session = spawn(command, cwd, cancel.clone(), timeout)?;
    let outcome = session.execute(cwd, input, model);
    let process_stopped = session.stop();
    let (status, output, error) = match outcome {
        Ok((status, output)) => (status, output, None),
        Err(error) => (
            if cancel.load(Ordering::Acquire) {
                "cancelled".into()
            } else {
                "failed".into()
            },
            None,
            Some(error),
        ),
    };
    Ok(RunnerResult {
        status: if process_stopped {
            status
        } else {
            "unknown".into()
        },
        output,
        error,
        process_stopped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }
    const COMPLETE: &str = r#"
while IFS= read -r line; do
 case "$line" in
 *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
 *'"method":"config/read"'*) printf '%s\n' '{"id":2,"result":{"config":{"model_provider":"loongport-task","shell_environment_policy":{"inherit":"none","set":{}}}}}' ;;
 *'"method":"thread/start"'*) printf '%s\n' '{"id":3,"result":{"thread":{"id":"thread-test","ephemeral":true}}}' ;;
 *'"method":"turn/start"'*)
   printf '%s\n' changed > result.txt
   printf '%s\n' '{"id":4,"result":{"turn":{"id":"turn-test"}}}'
   printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-test","turn":{"id":"turn-test","status":"completed","items":[{"id":"answer","type":"agentMessage","text":"Changed the file","phase":"final_answer"}]}}}' ;;
 esac
done
"#;
    #[test]
    fn protocol_completion_requires_correlated_turn_and_cleans_child() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(
            command(COMPLETE),
            dir.path(),
            "Change the file",
            Some("test-model"),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.output.as_deref(), Some("Changed the file"));
        assert!(result.process_stopped);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("result.txt")).unwrap(),
            "changed\n"
        );
    }
    #[test]
    fn terminal_failure_is_not_completion() {
        let dir = tempfile::tempdir().unwrap();
        let script = COMPLETE.replace("\"status\":\"completed\"", "\"status\":\"failed\"");
        let result = run(
            command(&script),
            dir.path(),
            "Change",
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(result.status, "failed");
        assert!(result.process_stopped);
    }
    #[test]
    fn cancellation_terminates_the_owned_process_group() {
        let dir = tempfile::tempdir().unwrap();
        // Replace the completed-event line with a child that remains alive until cancellation.
        let script = COMPLETE
            .lines()
            .map(|line| {
                if line.contains("turn/completed") {
                    "   sleep 60 & echo $! > descendant.pid; wait ;;"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let cwd = dir.path().to_owned();
        let worker = thread::spawn(move || {
            run(
                command(&script),
                &cwd,
                "Change",
                None,
                signal,
                Duration::from_secs(10),
            )
            .unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !dir.path().join("descendant.pid").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let pid: i32 = std::fs::read_to_string(dir.path().join("descendant.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        cancel.store(true, Ordering::Release);
        let result = worker.join().unwrap();
        assert_eq!(result.status, "cancelled");
        assert!(result.process_stopped);
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                Instant::now() < deadline,
                "descendant survived cancellation"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn blocked_stdin_is_cancelled_and_cleanup_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let script = r#"
IFS= read -r line
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r line
IFS= read -r line
printf '%s\n' '{"id":2,"result":{"config":{"model_provider":"loongport-task","shell_environment_policy":{"inherit":"none","set":{}}}}}'
IFS= read -r line
printf '%s\n' '{"id":3,"result":{"thread":{"id":"thread-test","ephemeral":true}}}'
sleep 60 & echo $! > descendant.pid
wait
"#;
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let cwd = dir.path().to_owned();
        let worker = thread::spawn(move || {
            run(
                command(script),
                &cwd,
                &"x".repeat(65536),
                Some("custom-model"),
                signal,
                Duration::from_secs(10),
            )
            .unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !dir.path().join("descendant.pid").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let started = Instant::now();
        cancel.store(true, Ordering::Release);
        let result = worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(result.status, "cancelled");
        assert!(result.process_stopped);
    }
    #[test]
    fn interactive_server_requests_are_rejected_and_never_approved() {
        let dir = tempfile::tempdir().unwrap();
        let script = COMPLETE.lines().map(|line| if line.contains("turn/completed") {
            r#"   printf '%s\n' '{"id":99,"method":"item/commandExecution/requestApproval","params":{}}' ;;"#
        } else { line }).collect::<Vec<_>>().join("\n");
        let result = run(
            command(&script),
            dir.path(),
            "Change",
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(
            result.error.as_deref(),
            Some("agents.runtime_requires_interaction")
        );
        assert_eq!(result.status, "failed");
        assert!(result.process_stopped);
    }
    #[test]
    fn inherited_mcp_configuration_is_rejected_before_any_turn() {
        let dir = tempfile::tempdir().unwrap();
        let script = COMPLETE.replace("\"config\":{\"model_provider\"", "\"config\":{\"mcp_servers\":{\"unexpected\":{\"command\":\"false\"}},\"model_provider\"");
        let result = run(
            command(&script),
            dir.path(),
            "Change",
            Some("custom-model"),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(
            result.error.as_deref(),
            Some("agents.runtime_unsafe_configuration")
        );
        assert_eq!(result.status, "failed");
        assert!(!dir.path().join("result.txt").exists());
        assert!(result.process_stopped);
    }
}
