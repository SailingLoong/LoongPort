//! Code-task lifecycle. Protocol details stay in the runner; checkout allocation stays in Workspace.
use super::{codex_runner::RunnerResult, workspace::Workspace};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeTaskRequest {
    pub request_id: String,
    pub repository: PathBuf,
    pub base_ref: String,
    pub input: String,
    pub model: Option<String>,
    pub credential_id: String,
    pub route_app: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTaskRecord {
    pub task_id: String,
    pub status: String,
    pub workspace: Option<PathBuf>,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub change_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub output_unavailable: bool,
    pub error: Option<String>,
    pub process_stopped: bool,
    pub can_cancel: bool,
    fingerprint: String,
}
struct Worker {
    record: Arc<Mutex<CodeTaskRecord>>,
    cancel: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
#[derive(Default)]
pub(super) struct CodeRuntime {
    workers: Mutex<HashMap<String, Worker>>,
    stopped: AtomicBool,
}

fn receipt_path(root: &Path, id: &str) -> PathBuf {
    root.join("claims").join(id).join("receipt.json")
}
fn persist(path: &Path, record: &CodeTaskRecord) -> Result<(), String> {
    let mut durable = record.clone();
    durable.output_unavailable |= durable.output.take().is_some();
    durable.can_cancel = false;
    crate::config::write_json_file_private(path, &durable)
        .map_err(|_| "agents.code_receipt_write_failed".into())
}
fn recovered(path: &Path) -> Result<CodeTaskRecord, String> {
    let bytes = std::fs::read(path).map_err(|_| "agents.code_receipt_unavailable")?;
    let mut record: CodeTaskRecord =
        serde_json::from_slice(&bytes).map_err(|_| "agents.code_receipt_invalid")?;
    if record.workspace.is_none() {
        if let Some(root) = path.ancestors().nth(3) {
            let allocated = root.join("workspaces").join(&record.task_id);
            if let Ok(workspace) = allocated.canonicalize() {
                record.workspace = Some(workspace);
            }
        }
    }
    record.can_cancel = false;
    if record.status == "accepted" {
        record.status = "unknown".into();
        record.error = Some("agents.code_interrupted_no_replay".into());
    }
    Ok(record)
}
impl CodeRuntime {
    pub(super) fn start<F>(
        &self,
        root: &Path,
        request: CodeTaskRequest,
        runner: F,
    ) -> Result<CodeTaskRecord, String>
    where
        F: FnOnce(&Path, &str, Option<&str>, Arc<AtomicBool>) -> Result<RunnerResult, String>
            + Send
            + 'static,
    {
        if self.stopped.load(Ordering::Acquire)
            || request.request_id.is_empty()
            || request.request_id.len() > 100
            || !request
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || request.input.trim().is_empty()
            || request.input.len() > 65536
        {
            return Err("agents.invalid_code_task".into());
        }
        let fingerprint = super::contract::digest(
            b"loongport-code-request-v1\0",
            &serde_json::to_vec(&request).map_err(|_| "agents.invalid_code_task")?,
        );
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| "agents.runtime_unavailable")?;
        if let Some(worker) = workers.get(&request.request_id) {
            let record = worker
                .record
                .lock()
                .map_err(|_| "agents.runtime_unavailable")?
                .clone();
            return if record.fingerprint == fingerprint {
                Ok(record)
            } else {
                Err("agents.request_conflict".into())
            };
        }
        let claims = root.join("claims");
        let workspaces = root.join("workspaces");
        crate::config::ensure_private_directory(&claims)
            .map_err(|_| "agents.workspace_unavailable")?;
        crate::config::ensure_private_directory(&workspaces)
            .map_err(|_| "agents.workspace_unavailable")?;
        let claim = claims.join(&request.request_id);
        let receipt = receipt_path(root, &request.request_id);
        match std::fs::create_dir(&claim) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let record = recovered(&receipt)?;
                return if record.fingerprint == fingerprint {
                    Ok(record)
                } else {
                    Err("agents.request_conflict".into())
                };
            }
            Err(_) => return Err("agents.workspace_claim_failed".into()),
        }
        let record = CodeTaskRecord {
            task_id: request.request_id.clone(),
            status: "accepted".into(),
            workspace: None,
            base_commit: None,
            head_commit: None,
            change_summary: None,
            output: None,
            output_unavailable: false,
            error: None,
            process_stopped: false,
            can_cancel: true,
            fingerprint,
        };
        persist(&receipt, &record)?;
        let record = Arc::new(Mutex::new(record));
        let cancel = Arc::new(AtomicBool::new(false));
        let result = record.clone();
        let signal = cancel.clone();
        let id = request.request_id.clone();
        let failure_receipt = receipt.clone();
        let handle = thread::Builder::new()
            .name("loongport-code-task".into())
            .spawn(move || {
                let outcome = (|| -> Result<RunnerResult, String> {
                    if signal.load(Ordering::Acquire) {
                        return Err("agents.cancelled".into());
                    }
                    let workspace = Workspace::create(
                        &request.repository,
                        &request.base_ref,
                        &workspaces.join(&request.request_id),
                    )?;
                    {
                        let mut record = result.lock().map_err(|_| "agents.runtime_unavailable")?;
                        record.workspace = Some(workspace.path.clone());
                        record.base_commit = Some(workspace.base_commit.clone());
                        persist(&receipt, &record)?;
                    }
                    let execution = if signal.load(Ordering::Acquire) {
                        Err("agents.cancelled".into())
                    } else {
                        runner(
                            &workspace.path,
                            &request.input,
                            request.model.as_deref(),
                            signal.clone(),
                        )
                    };
                    let (head, summary) = workspace.inspect()?;
                    let mut record = result.lock().map_err(|_| "agents.runtime_unavailable")?;
                    record.head_commit = Some(head);
                    record.change_summary = Some(summary);
                    execution
                })();
                if let Ok(mut record) = result.lock() {
                    record.can_cancel = false;
                    match outcome {
                        Ok(outcome) => {
                            record.status = outcome.status;
                            record.output = outcome.output;
                            record.error = outcome.error;
                            record.process_stopped = outcome.process_stopped;
                        }
                        Err(error) => {
                            record.status = if signal.load(Ordering::Acquire) {
                                "cancelled".into()
                            } else {
                                "failed".into()
                            };
                            record.error = Some(error);
                        }
                    }
                    if persist(&receipt, &record).is_err() {
                        record.status = "unknown".into();
                        record.error = Some("agents.code_receipt_write_failed".into());
                    }
                }
            });
        match handle {
            Ok(handle) => {
                workers.insert(
                    id,
                    Worker {
                        record: record.clone(),
                        cancel,
                        handle: Some(handle),
                    },
                );
            }
            Err(_) => {
                let mut value = record.lock().map_err(|_| "agents.runtime_unavailable")?;
                value.status = "failed".into();
                value.can_cancel = false;
                value.process_stopped = true;
                value.error = Some("agents.code_worker_start_failed".into());
                persist(&failure_receipt, &value)?;
            }
        }
        let value = record
            .lock()
            .map_err(|_| "agents.runtime_unavailable")?
            .clone();
        Ok(value)
    }
    pub(super) fn list(&self, root: &Path) -> Result<Vec<CodeTaskRecord>, String> {
        let workers = self
            .workers
            .lock()
            .map_err(|_| "agents.runtime_unavailable")?;
        let mut records = Vec::new();
        let entries = match std::fs::read_dir(root.join("claims")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(_) => return Err("agents.code_receipt_unavailable".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "agents.code_receipt_unavailable")?;
            let id = entry.file_name().to_string_lossy().into_owned();
            let record = if let Some(worker) = workers.get(&id) {
                worker
                    .record
                    .lock()
                    .map_err(|_| "agents.runtime_unavailable")?
                    .clone()
            } else {
                recovered(&entry.path().join("receipt.json"))?
            };
            records.push(record);
        }
        records.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(records)
    }
    pub(super) fn cancel(&self, id: &str) -> Result<(), String> {
        let workers = self
            .workers
            .lock()
            .map_err(|_| "agents.runtime_unavailable")?;
        let worker = workers.get(id).ok_or("agents.task_unavailable")?;
        worker.cancel.store(true, Ordering::Release);
        Ok(())
    }
    pub(super) fn stop(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        let workers = std::mem::take(
            &mut *self
                .workers
                .lock()
                .map_err(|_| "agents.runtime_unavailable")?,
        );
        for worker in workers.values() {
            worker.cancel.store(true, Ordering::Release);
        }
        for (_, mut worker) in workers {
            if let Some(handle) = worker.handle.take() {
                handle.join().map_err(|_| "agents.code_worker_failed")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repository(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "base",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        repo
    }
    #[test]
    fn completed_code_tasks_are_not_replayed_after_owner_restart() {
        let root = tempfile::tempdir().unwrap();
        let request = CodeTaskRequest {
            request_id: "code-test".into(),
            repository: repository(root.path()),
            base_ref: "HEAD".into(),
            input: "Write a file".into(),
            model: None,
            credential_id: "test-ref".into(),
            route_app: "codex".into(),
        };
        let runtime = CodeRuntime::default();
        let receipts = root.path().join("tasks");
        runtime
            .start(&receipts, request.clone(), |cwd, _, _, _| {
                std::fs::write(cwd.join("result.txt"), "changed").unwrap();
                Ok(RunnerResult {
                    status: "completed".into(),
                    output: Some("done".into()),
                    error: None,
                    process_stopped: true,
                })
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while runtime.list(&receipts).unwrap()[0].status == "accepted" {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        runtime.stop().unwrap();
        let reopened = CodeRuntime::default();
        let record = reopened
            .start(&receipts, request, |_, _, _, _| panic!("replayed"))
            .unwrap();
        assert_eq!(record.status, "completed");
        assert!(record.output_unavailable);
        assert!(record.output.is_none());
        assert!(record
            .change_summary
            .as_deref()
            .unwrap()
            .contains("result.txt"));
        assert!(!record.can_cancel);
    }
}
