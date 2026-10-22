//! Original native task owner and local IPC boundary. No prompt or output persistence.
//! Reuses the existing Database, Vault, request router and usage log.
pub(crate) mod bindings;
pub(crate) mod contract;
pub(crate) mod execution;
pub(crate) mod snapshot;
#[cfg(unix)]
pub(crate) mod tasks;
use crate::error::AppError;
#[cfg(any(unix, test))]
use crate::{app_config::AppType, store::AppState};
#[cfg(any(unix, test))]
use bindings::{self as ledger, id, Scope};
#[cfg(any(unix, test))]
use rusqlite::{params, OptionalExtension};
#[cfg(any(unix, test))]
use serde::Deserialize;
use serde_json::{json, Value};

use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};
#[cfg(any(unix, test))]
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
fn unavailable() -> AppError {
    AppError::Config("paperclip.owner_unavailable".into())
}
#[cfg(any(unix, test))]
#[derive(Clone)]
pub(crate) struct Owner {
    state: AppState,
    app: AppType,
    owner: String,
    control: Arc<Control>,
    runtime: Option<tokio::runtime::Handle>,
    #[cfg(all(test, unix))]
    synthetic_responses_fixture: bool,
}
#[cfg(any(unix, test))]
struct Saved {
    scope: String,
    binding: String,
    phase: String,
    receipt: Option<String>,
}
#[cfg(any(unix, test))]
impl Owner {
    pub(crate) fn new(state: AppState, app: AppType, owner: String) -> Result<Self, AppError> {
        if !id(&owner) {
            return Err(unavailable());
        }
        state
            .db
            .secret_session()
            .ensure_available()
            .map_err(|_| unavailable())?;
        let owner = Self {
            state,
            app,
            owner,
            control: Arc::new(Control::default()),
            runtime: tokio::runtime::Handle::try_current().ok(),
            #[cfg(all(test, unix))]
            synthetic_responses_fixture: false,
        };
        {
            let conn = owner.db_admission(Instant::now() + REQUEST_BUDGET)?;
            ledger::read_rows(&conn)?;
        }
        Ok(owner)
    }
    #[cfg(all(test, unix))]
    pub(crate) fn with_synthetic_responses_fixture(mut self) -> Self {
        self.synthetic_responses_fixture = true;
        self
    }
    fn admitted(&self, deadline: Instant) -> Result<(), AppError> {
        if self.control.stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            Err(unavailable())
        } else {
            Ok(())
        }
    }
    fn db_admission(
        &self,
        deadline: Instant,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, AppError> {
        loop {
            self.admitted(deadline)?;
            if let Some(conn) = self
                .state
                .db
                .conn
                .paperclip_try_lock()
                .map_err(|_| unavailable())?
            {
                self.admitted(deadline)?;
                return Ok(conn);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn sync_admission(
        &self,
        deadline: Instant,
    ) -> Result<tokio::sync::MutexGuard<'static, ()>, AppError> {
        loop {
            self.admitted(deadline)?;
            if let Ok(guard) = crate::services::sync_protocol::sync_mutex().try_lock() {
                self.admitted(deadline)?;
                return Ok(guard);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Result<Value, AppError> {
        self.snapshot_until(Instant::now() + REQUEST_BUDGET)
    }
    fn snapshot_until(&self, deadline: Instant) -> Result<Value, AppError> {
        self.admitted(deadline)?;
        let identity = self
            .state
            .proxy_service
            .request_identity(self.app.as_str())?;
        let c = self.db_admission(deadline)?;
        let value = snapshot::snapshot_on_connection(
            &c,
            self.app.as_str(),
            &self.owner,
            identity.fingerprint(),
        )?;
        self.admitted(deadline)?;
        Ok(value)
    }
    fn saved(&self, conn: &rusqlite::Connection, s: &Scope) -> Result<Option<Saved>, AppError> {
        let saved:Option<Saved>=conn.query_row("SELECT scope,binding,phase,receipt FROM paperclip_run_bindings WHERE owner=?1 AND app=?2 AND company=?3 AND run=?4",params![self.owner,self.app.as_str(),s.company_id,s.run_id],|r|Ok(Saved{scope:r.get(0)?,binding:r.get(1)?,phase:r.get(2)?,receipt:r.get(3)?})).optional().map_err(|_|unavailable())?;
        if let Some(row) = &saved {
            ledger::validate_row(
                &self.owner,
                self.app.as_str(),
                &s.company_id,
                &s.run_id,
                &row.scope,
                &row.binding,
                &row.phase,
                row.receipt.as_deref(),
            )?;
        }
        Ok(saved)
    }
    fn scope(&self, v: Value) -> Result<Scope, AppError> {
        let s: Scope = serde_json::from_value(v).map_err(|_| unavailable())?;
        if !s.valid() || s.owner_id != self.owner {
            return Err(unavailable());
        }
        Ok(s)
    }
    fn current(&self, conn: &rusqlite::Connection, s: &Scope) -> Result<(), AppError> {
        let identity = self
            .state
            .proxy_service
            .request_identity(self.app.as_str())?;
        let v = snapshot::snapshot_on_connection(
            conn,
            self.app.as_str(),
            &self.owner,
            identity.fingerprint(),
        )?;
        if v["revision"] != s.snapshot_revision
            || s.resource_id != snapshot::resource_id(&self.owner, self.app.as_str())
            || v["resources"][0]["nativeSupport"] != json!(true)
        {
            return Err(unavailable());
        }
        Ok(())
    }
    #[cfg(unix)]
    pub(crate) fn listen(&self, root: &Path) -> Result<Bridge, AppError> {
        self.admitted(Instant::now() + REQUEST_BUDGET)?;
        let meta = std::fs::symlink_metadata(root).map_err(|_| unavailable())?;
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || std::fs::canonicalize(root).map_err(|_| unavailable())? != root
        {
            return Err(unavailable());
        }
        let socket = root.join("owner.sock");
        if let Ok(old) = std::fs::symlink_metadata(&socket) {
            use std::os::unix::fs::FileTypeExt;
            if !old.file_type().is_socket() || old.uid() != meta.uid() || old.mode() & 0o077 != 0 {
                return Err(unavailable());
            }
            match UnixStream::connect(&socket) {
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                _ => return Err(unavailable()),
            }
            let now = std::fs::symlink_metadata(&socket).map_err(|_| unavailable())?;
            if (old.dev(), old.ino()) != (now.dev(), now.ino()) {
                return Err(unavailable());
            }
            std::fs::remove_file(&socket).map_err(|_| unavailable())?;
        }
        let listener = UnixListener::bind(&socket).map_err(|_| unavailable())?;
        let sm = std::fs::symlink_metadata(&socket).map_err(|_| unavailable())?;
        let mut bridge = Bridge {
            control: self.control.clone(),
            worker: None,
            socket,
            identity: (sm.dev(), sm.ino()),
        };
        std::fs::set_permissions(&bridge.socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| unavailable())?;
        listener.set_nonblocking(true).map_err(|_| unavailable())?;
        self.recover_execution()?;
        let owner = self.clone();
        bridge.worker = Some(
            thread::Builder::new()
                .name("paperclip-metadata".into())
                .spawn(move || {
                    while !owner.control.stop.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                // macOS accept inherits listener O_NONBLOCK. The per-frame
                                // absolute deadline requires blocking reads with timeouts.
                                if stream.set_nonblocking(false).is_err() {
                                    continue;
                                }
                                let Ok(active) = stream.try_clone() else {
                                    continue;
                                };
                                if let Ok(mut slot) = owner.control.active.lock() {
                                    *slot = Some(active);
                                } else {
                                    break;
                                }
                                if !owner.control.stop.load(Ordering::Acquire) {
                                    owner.respond(stream);
                                }
                                if let Ok(mut slot) = owner.control.active.lock() {
                                    slot.take();
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(2))
                            }
                            Err(_) => break,
                        }
                    }
                })
                .map_err(|_| unavailable())?,
        );
        Ok(bridge)
    }
    #[cfg(unix)]
    fn respond(&self, mut stream: UnixStream) {
        let deadline = Instant::now() + REQUEST_BUDGET;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if self.admitted(deadline).is_err() {
                return;
            }
            if stream
                .set_read_timeout(Some(deadline.saturating_duration_since(Instant::now())))
                .is_err()
            {
                return;
            }
            let n = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.len() > 65536 {
                return;
            }
            if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
                if end + 1 != bytes.len() {
                    return;
                }
                break;
            }
        }
        let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
            return;
        };
        if request.version != 1 || !id(&request.id) {
            return;
        }
        let result = match request.method.as_str() {
            "snapshot" if request.params.as_object().is_some_and(|o| o.is_empty()) => {
                self.snapshot_until(deadline)
            }
            "bindExecution" => self.bind_execution_until(request.params, deadline),
            "startExecution" => self.start_execution_until(request.params, deadline),
            "result" => self.execution_result_until(request.params, deadline),
            "cancelExecution" => self.cancel_execution_until(request.params, deadline),
            _ => Err(unavailable()),
        };
        if self.admitted(deadline).is_err() {
            return;
        }
        let response = match result {
            Ok(result) => json!({"version":1,"id":request.id,"ok":true,"result":result}),
            Err(_) => json!({"version":1,"id":request.id,"ok":false,"error":"request_denied"}),
        };
        if let Ok(mut data) = serde_json::to_vec(&response) {
            data.push(b'\n');
            if data.len() <= 65536
                && stream
                    .set_write_timeout(Some(deadline.saturating_duration_since(Instant::now())))
                    .is_ok()
            {
                let _ = stream.write_all(&data);
            }
        }
    }
}
#[cfg(any(unix, test))]
const REQUEST_BUDGET: Duration = Duration::from_secs(1);
#[cfg(any(unix, test))]
#[derive(Default)]
struct Control {
    #[cfg(unix)]
    local_task_gate: Mutex<()>,
    #[cfg(unix)]
    // Ephemeral gap before the dependent binding exists: None is pending, Some is a stop reason.
    local_followups: Mutex<std::collections::HashMap<String, Option<&'static str>>>,
    #[cfg(unix)]
    local_summary_tasks: Mutex<std::collections::HashMap<String, contract::Task>>,
    stop: AtomicBool,
    #[cfg(unix)]
    active: Mutex<Option<UnixStream>>,
    executions: execution::Executions,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(unix)]
struct Request {
    version: u8,
    id: String,
    method: String,
    params: Value,
}
#[cfg(unix)]
pub(crate) struct Bridge {
    control: Arc<Control>,
    worker: Option<thread::JoinHandle<()>>,
    socket: PathBuf,
    identity: (u64, u64),
}
#[cfg(unix)]
impl Bridge {
    fn shutdown(&mut self) -> Result<(), AppError> {
        self.control.stop.store(true, Ordering::Release);
        if let Ok(slot) = self.control.active.lock() {
            if let Some(stream) = slot.as_ref() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        self.control.executions.stop()?;
        // No GUI callbacks in worker. Join is the old-worker barrier before unlink/relisten.
        let result = if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| unavailable())
        } else {
            Ok(())
        };
        if std::fs::symlink_metadata(&self.socket)
            .is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
        {
            std::fs::remove_file(&self.socket).map_err(|_| unavailable())?;
        }
        result
    }
    pub(crate) fn stop(mut self) -> Result<(), AppError> {
        self.shutdown()
    }
}
#[cfg(unix)]
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(all(unix, not(test)))]
fn fault(_stage: &str) {}
#[cfg(test)]
fn fault(stage: &str) {
    if let Ok(mode) = std::env::var("PC_NATIVE_FAULT") {
        if mode == format!("crash-{stage}")
            || (mode == "synthetic-responses-crash" && stage == "execution-claimed")
        {
            std::process::exit(79);
        }
        if mode == format!("hang-{stage}") {
            thread::sleep(Duration::from_secs(3));
        }
    }
}
pub(crate) use execution::RouteBinding;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{database::Database, provider::Provider};
    pub(crate) fn fixture() -> (tokio::runtime::Runtime, AppState, Owner) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let entered = rt.enter();
        let db = Arc::new(Database::memory().unwrap());
        db.save_provider(
            "codex",
            &Provider::with_id("synthetic".into(), "Synthetic".into(), json!({}), None),
        )
        .unwrap();
        let state = AppState::new(db).unwrap();
        let owner = Owner::new(state.clone(), AppType::Codex, "owner-a".into()).unwrap();
        drop(entered);
        (rt, state, owner)
    }
    pub(crate) fn scope(owner: &Owner) -> Value {
        let v = owner.snapshot().unwrap();
        json!({"companyId":"company-a","taskId":"task-a","agentId":"agent-a","runId":"run-a","resourceId":v["resources"][0]["resourceId"],"ownerId":"owner-a","snapshotRevision":v["revision"]})
    }
    pub(crate) fn task() -> contract::Task {
        serde_json::from_value(json!({"version":3,"input":"synthetic request","model":"gpt-4.1","maxOutputTokens":32,"timeoutMs":1000,"budget":{"mode":"estimated","currency":"USD","limitMicros":1000}})).unwrap()
    }
    #[cfg(unix)]
    pub(crate) fn gated_shutdown_fixture(
        root: &std::path::Path,
    ) -> (
        Bridge,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let socket = root.join("owner.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let meta = std::fs::symlink_metadata(&socket).unwrap();
        let control = Arc::new(Control::default());
        let (release, wait) = std::sync::mpsc::channel();
        let (stopped, observe) = std::sync::mpsc::channel();
        let watching = control.clone();
        let worker = std::thread::spawn(move || {
            let _owned = listener;
            while !watching.stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
            let _ = stopped.send(());
            let _ = wait.recv();
        });
        (
            Bridge {
                control,
                worker: Some(worker),
                socket,
                identity: (meta.dev(), meta.ino()),
            },
            release,
            observe,
        )
    }

    // Locked tempfile defaults to ordinary directory permissions, not private UDS roots.
    #[cfg(unix)]
    pub(crate) fn private_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
            .unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn private_fixture_root_is_physical_private_and_inspectable() {
        let root = private_root();
        assert_eq!(root.path(), root.path().canonicalize().unwrap());
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let device = crate::live::engine::DeviceStore::at(root.path().join("device"));
        crate::secrets::upgrade::inspect(root.path(), &device).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn ordinary_permission_directory_is_rejected() {
        let (_rt, _state, owner) = fixture();
        let root = private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(owner.listen(&path).is_err());
        assert!(!path.join("owner.sock").exists());
    }
    #[test]
    fn binding_idempotence_rejects_changed_task_without_persisting_input() {
        let (_rt, state, owner) = fixture();
        let scope = scope(&owner);
        let t = task();
        let b = owner
            .bind_execution(json!({"scope":scope,"task":t}))
            .unwrap();
        assert_eq!(
            owner
                .bind_execution(json!({"scope":scope,"task":t}))
                .unwrap(),
            b
        );
        let mut changed = t.clone();
        changed.input.push('x');
        assert!(owner
            .bind_execution(json!({"scope":scope,"task":changed}))
            .is_err());
        let c = state.db.conn.lock().unwrap();
        let raw: String = c
            .query_row("SELECT binding FROM paperclip_run_bindings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!raw.contains(&t.input));
    }
    #[test]
    fn claimed_run_is_never_replayed_after_recovery() {
        let (_rt, state, owner) = fixture();
        let t = task();
        let b = owner
            .bind_execution(json!({"scope":scope(&owner),"task":t}))
            .unwrap();
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE paperclip_run_bindings SET phase='accepted'", [])
            .unwrap();
        owner.recover_execution().unwrap();
        let result = owner
            .start_execution_with_until(
                json!({"binding":b,"task":t}),
                |_, _| async { panic!("replay") },
                Instant::now() + REQUEST_BUDGET,
            )
            .unwrap();
        assert_eq!(result["status"], "unknown");
    }
    #[test]
    fn completed_lost_output_keeps_usage_and_blocks_replay() {
        let (_rt, state, owner) = fixture();
        let t = task();
        let b = owner
            .bind_execution(json!({"scope":scope(&owner),"task":t}))
            .unwrap();
        let mut r = contract::outcome(&b, "completed");
        r["providerWorkStarted"] = json!(true);
        r["outputDigest"] = json!("a".repeat(64));
        {
            let c = state.db.conn.lock().unwrap();
            c.execute("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,latency_ms,status_code,session_id,created_at) VALUES('original-row','synthetic','codex','gpt-4.1',1,200,?1,1)",[execution::usage_session_for_binding(b["bindingId"].as_str().unwrap()).unwrap()]).unwrap();
        }
        let row = state
            .db
            .get_request_detail_raw("original-row")
            .unwrap()
            .unwrap();
        r["usage"] = json!({"requestId":"original-row","digest":contract::digest(b"paperclip-original-usage-v3\0",&serde_json::to_vec(&row).unwrap()),"currency":"USD"});
        let r = contract::durable_result(&r);
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE paperclip_run_bindings SET phase='completed',receipt=?1",
                [r.to_string()],
            )
            .unwrap();
        let result = owner.execution_result(b.clone()).unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(result["outputUnavailable"], true);
        assert_eq!(result["usage"]["requestId"], "original-row");
        assert_eq!(
            owner
                .start_execution_with_until(
                    json!({"binding":b,"task":t}),
                    |_, _| async { panic!("replay") },
                    Instant::now() + REQUEST_BUDGET
                )
                .unwrap(),
            result
        );
    }
}
