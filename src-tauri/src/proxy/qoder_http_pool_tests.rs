//! Product-router HTTP-worker regressions. Rust compilation/tests: NOTRUN.
//! Register only at server.rs root with #[cfg(test)] #[path = "qoder_http_pool_tests.rs"].
use super::*;
use crate::provider::{Provider, ProviderMeta};
use crate::services::provider::qoder_daily::http::{CredentialSource, PatDeclaration};
use crate::services::provider::qoder_http_auth::{
    Established, IdentityAction, IdentityActionRequest, PreparedIdentityAction, TokenPair,
};
use axum::http::StatusCode;
use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify};
use tower::ServiceExt;

const MAX_CAPTURE: usize = 2 * 1024 * 1024;
const MODEL: &str = "fixture-model";
const TOOL_ID: &str = "call_http_fixture";
const PRIVATE_TAIL: &str = "REVOKED_PRIVATE_TAIL_MUST_NOT_ESCAPE";
const WORKER_SHA256: &str = "7b25864711ede8e114721dfc7aed6aa327d1b92e79d21937b8a90fdb9da20fd6";
const PROTOCOL_SHA256: &str = "af35fc63e2b7b17fc9ee838b67a9ab76e5cb2e40f6aeda5d9ff30f99533aa284";

#[derive(Clone, Copy)]
enum Reply {
    Plain,
    ToolsThenText,
    Unknown,
    PartialFailure,
    Hold,
    RevocableTail,
    AuthRejected,
    BusinessRejected,
}
struct Upstream {
    posts: Mutex<Vec<String>>,
    posted: Notify,
    closed: AtomicUsize,
    disconnected: Notify,
    release_tail: Notify,
    tasks: StdMutex<Vec<tokio::task::AbortHandle>>,
}
impl Upstream {
    async fn wait_posts(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let notified = self.posted.notified();
                if self.posts.lock().await.len() >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("real loopback POST must arrive");
    }
    async fn wait_closed(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let notified = self.disconnected.notified();
                if self.closed.load(Ordering::SeqCst) >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("worker cancellation/drop must close the independently owned upstream socket");
    }
}
struct ClosedPost(Arc<Upstream>);
impl Drop for ClosedPost {
    fn drop(&mut self) {
        self.0.closed.fetch_add(1, Ordering::SeqCst);
        self.0.disconnected.notify_one();
    }
}
fn envelope(body: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"statusCodeValue":200,"body":body.to_string()})
    )
}
fn delta(value: Value, finish: Value) -> String {
    envelope(json!({"choices":[{"index":0,"delta":value,"finish_reason":finish}]}))
}
fn done() -> String {
    format!(
        "data: {}\n\n",
        json!({"statusCodeValue":200,"body":"[DONE]"})
    )
}
fn plain(text: &str) -> String {
    delta(json!({"content":text}), Value::Null) + &delta(json!({}), json!("stop")) + &done()
}
fn tool_reply() -> String {
    delta(
        json!({"content":"tool-preface-with-annotations"}),
        Value::Null,
    ) + &delta(
        json!({"tool_calls":[{"index":0,"id":TOOL_ID,"type":"function","function":{"name":"fixture_read","arguments":"{\"path\":\"fixture.txt\"}"}}]}),
        Value::Null,
    ) + &delta(json!({}), json!("tool_calls"))
        + &done()
}

// This is only the synthetic Qoder upstream, not a substitute application router.
// The actual tested request traverses ProxyServer.build_router unchanged.
async fn serve_connection(mut socket: tokio::net::TcpStream, state: Arc<Upstream>, reply: Reply) {
    let mut bytes = Vec::new();
    let end;
    loop {
        let mut part = [0u8; 4096];
        let n = socket.read(&mut part).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&part[..n]);
        assert!(bytes.len() <= MAX_CAPTURE, "bounded synthetic request");
        if let Some(index) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            end = index + 4;
            break;
        }
    }
    let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let method = header.split_whitespace().next().unwrap();
    let path = header.split_whitespace().nth(1).unwrap();
    let header_value = |key: &str| -> Option<String> {
        header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.trim().to_owned())
    };
    assert!(header_value("authorization").is_some_and(|value| value.starts_with("Bearer COSY.")));
    assert!(header_value("cosy-machineid").is_some());
    let length = header_value("content-length")
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(end + length <= MAX_CAPTURE);
    while bytes.len() < end + length {
        let mut part = [0u8; 4096];
        let n = socket.read(&mut part).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&part[..n]);
    }
    if method == "GET" {
        if header_value("cosy-clienttype").as_deref() == Some("5") {
            assert_eq!(path, "/algo/api/v2/model/list");
        } else {
            assert_eq!(path, "/algo/api/v2/model/list?Encode=1");
        }
        let body = json!({"chat":[{"key":MODEL,"source":"fixture","enable":true,"max_input_tokens":100000}]}).to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        let _ = socket.shutdown().await;
        return;
    }
    assert_eq!(method, "POST");
    assert_eq!(path, "/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1");
    let uid = header_value("cosy-user").unwrap();
    let account = uid
        .strip_prefix("synthetic-http-uid:")
        .expect("synthetic UID only")
        .to_owned();
    let number = {
        let mut posts = state.posts.lock().await;
        posts.push(account);
        posts.len()
    };
    state.posted.notify_one();
    let _closed = ClosedPost(state.clone());
    if matches!(reply, Reply::Unknown) {
        return;
    }
    if matches!(reply, Reply::BusinessRejected) {
        let body = r#"{"code":110,"message":"synthetic quota refusal"}"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let _ = socket.shutdown().await;
        return;
    }
    if matches!(reply, Reply::AuthRejected) {
        socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let _ = socket.shutdown().await;
        return;
    }
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = match reply {
        Reply::Plain => plain("ordered-history-and-auth-ok"),
        Reply::ToolsThenText if number == 1 => tool_reply(),
        Reply::ToolsThenText => plain("tool-result-summary"),
        Reply::PartialFailure => {
            delta(json!({"content":"partial-before-failure"}), Value::Null)
                + &format!(
                    "data: {}\n\n",
                    json!({"statusCodeValue":503,"body":"{\"message\":\"PRIVATE_UPSTREAM_ERROR\"}"})
                )
        }
        Reply::Hold | Reply::RevocableTail => {
            socket
                .write_all(delta(json!({"content":"visible-prefix"}), Value::Null).as_bytes())
                .await
                .unwrap();
            let mut byte = [0u8; 1];
            tokio::select! {
                _ = state.release_tail.notified() => {
                    if matches!(reply, Reply::RevocableTail) {
                        let _ = socket.write_all(plain(PRIVATE_TAIL).as_bytes()).await;
                    }
                }
                _ = socket.read(&mut byte) => {}
            }
            let _ = socket.shutdown().await;
            return;
        }
        Reply::Unknown | Reply::AuthRejected | Reply::BusinessRejected => unreachable!(),
    };
    let _ = socket.write_all(answer.as_bytes()).await;
    let _ = socket.shutdown().await;
}

struct HttpFixture {
    proxy: ProxyServer,
    db: Arc<Database>,
    runtime: crate::proxy::qoder_host::HostConfig,
    directory: tempfile::TempDir,
    upstream: Arc<Upstream>,
    listener: tokio::task::JoinHandle<()>,
    _home: crate::secrets::testing::TestHome,
}
impl HttpFixture {
    async fn new(accounts: &[(&str, bool)], reply: Reply) -> Self {
        Self::new_with_terminal_fault(accounts, reply, None).await
    }
    async fn new_with_terminal_fault(
        accounts: &[(&str, bool)],
        reply: Reply,
        terminal_fault: Option<&str>,
    ) -> Self {
        Self::new_with_source(accounts, reply, terminal_fault, "qoder_sdk").await
    }
    async fn new_with_source(
        accounts: &[(&str, bool)],
        reply: Reply,
        terminal_fault: Option<&str>,
        source: &str,
    ) -> Self {
        Self::new_with_refresh(accounts, reply, terminal_fault, source, false).await
    }
    async fn new_with_refresh(
        accounts: &[(&str, bool)],
        reply: Reply,
        terminal_fault: Option<&str>,
        source: &str,
        refreshable: bool,
    ) -> Self {
        let home = crate::secrets::testing::TestHome::new().unwrap();
        assert!(
            std::env::var_os("CC_SWITCH_TEST_HOME").is_some(),
            "isolated original test home required"
        );
        let root = std::env::var_os("QODER_HOST_PRODUCT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .to_owned()
            });
        let worker = root.join("scripts/qoder/worker.mjs");
        let protocol = root.join("scripts/qoder/http-protocol.mjs");
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&worker).unwrap())),
            WORKER_SHA256
        );
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&protocol).unwrap())),
            PROTOCOL_SHA256
        );
        let wrapper = root.join("tests/qoder-runtime/fixtures/http-proxy-wrapper.mjs");
        assert!(wrapper.is_file());
        let directory = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let now = chrono::Utc::now().timestamp_millis();
        for (position, (id, authorized)) in accounts.iter().enumerate() {
            let mut provider = Provider::with_id(
                (*id).into(),
                (*id).into(),
                json!({
                    "env":{"OPENAI_API_KEY":format!("synthetic-original-pat:{id}")},
                    "config":"model_provider = \"qoder\"\n[model_providers.qoder]\nbase_url = \"http://127.0.0.1:9\"\nwire_api = \"responses\"\n"
                }),
                None,
            );
            if source == "qoder_cn_device" {
                provider
                    .settings_config
                    .as_object_mut()
                    .unwrap()
                    .remove("env");
            }
            provider.sort_index = Some(position);
            provider.meta = Some(ProviderMeta {
                provider_type: Some(source.into()),
                local_proxy_request_overrides: (*id == "qoder-incompatible").then(|| {
                    crate::provider::LocalProxyRequestOverrides {
                        body: Some(json!({"tools":[{"type":"custom","name":"unsupported"}]})),
                        ..Default::default()
                    }
                }),
                ..Default::default()
            });
            db.save_provider("codex", &provider).unwrap();
            db.add_to_failover_queue("codex", id).unwrap();
            if *authorized {
                let saved = db.get_provider_by_id(id, "codex").unwrap().unwrap();
                let declaration = PatDeclaration {
                    provider_id: (*id).into(),
                    credential_generation: saved
                        .meta
                        .unwrap()
                        .qoder_campaign_state
                        .unwrap()
                        .generation,
                    source: match source {
                        "qoder_cn_device" => CredentialSource::ChinaQoderDevice,
                        "qoder_cn" => CredentialSource::ChinaQoderPat,
                        _ => CredentialSource::InternationalQoderPat,
                    },
                };
                let request = IdentityActionRequest {
                    declaration,
                    action: IdentityAction::Establish,
                };
                let prepared = db
                    .qoder_http_prepare_identity_action(&request, now)
                    .unwrap();
                let PreparedIdentityAction::Establish { receipt, .. } = prepared else {
                    panic!("fresh fixture must prepare identity establishment");
                };
                let lease = db.qoder_http_reserve_identity_action(receipt).unwrap();
                db.qoder_http_publish_establish(
                    &lease,
                    Established {
                        uid: format!("synthetic-http-uid:{id}"),
                        tokens: TokenPair {
                            access_token: format!("synthetic-http-access:{id}"),
                            refresh_token: if refreshable {
                                "synthetic-http-refresh".into()
                            } else {
                                String::new()
                            },
                            expires_at_ms: now + 3_600_000,
                        },
                    },
                    now,
                )
                .unwrap();
            }
        }
        db.set_current_provider("codex", accounts[0].0).unwrap();
        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        config.max_retries = 2;
        config.streaming_first_byte_timeout = 3;
        config.streaming_idle_timeout = 3;
        config.non_streaming_timeout = 5;
        db.update_proxy_config_for_app(config).await.unwrap();
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            db.clone(),
            #[cfg(feature = "gui")]
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        let upstream = Arc::new(Upstream {
            posts: Mutex::new(Vec::new()),
            posted: Notify::new(),
            closed: AtomicUsize::new(0),
            disconnected: Notify::new(),
            release_tail: Notify::new(),
            tasks: StdMutex::new(Vec::new()),
        });
        let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let origin = format!("http://{}/", socket.local_addr().unwrap());
        let state = upstream.clone();
        let listener = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = socket.accept().await else {
                    break;
                };
                let state_for_task = state.clone();
                let task = tokio::spawn(serve_connection(socket, state_for_task, reply));
                state.tasks.lock().unwrap().push(task.abort_handle());
            }
        });
        let private_wrapper = directory.path().join("worker.mjs");
        // Every path/origin is a trusted test constant, not HTTP input or product config.
        let diagnostic_path = directory.path().join("diagnostic.jsonl");
        std::fs::write(&private_wrapper, format!(r#"import {{pathToFileURL}} from 'node:url';
import {{appendFileSync}} from 'node:fs';
const diagnosticPath={};
const diagnostic=value=>appendFileSync(diagnosticPath,JSON.stringify(value)+'\n');
diagnostic({{stage:'bootstrap'}});
try {{
  const {{runFixture}}=await import(pathToFileURL({}).href);
  await runFixture({});
}} catch (error) {{
  const allowed=['ERR_MODULE_NOT_FOUND','ERR_UNSUPPORTED_ESM_URL_SCHEME','EACCES','ENOENT','EPIPE'];
  const errorClass=allowed.includes(error?.code)?error.code:'other';
  diagnostic({{stage:'bootstrapFailed',errorClass}});
  throw error;
}}
"#, serde_json::to_string(&diagnostic_path).unwrap(), serde_json::to_string(&wrapper).unwrap(),
            json!({"origin":origin,"tracePath":directory.path().join("trace.jsonl"),"workerPath":worker,"terminalFault":terminal_fault,"diagnosticPath":diagnostic_path}))).unwrap();
        let runtime = crate::proxy::qoder_host::HostConfig {
            node: std::env::var_os("QODER_HOST_NODE")
                .unwrap_or_else(|| "node".into())
                .into(),
            worker: private_wrapper,
        };
        Self {
            _home: home,
            proxy,
            db,
            runtime,
            directory,
            upstream,
            listener,
        }
    }
    async fn request(&self, endpoint: &str, body: Value) -> axum::response::Response {
        self.proxy
            .build_router()
            .layer(axum::Extension(self.runtime.clone()))
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(endpoint)
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer ignored-caller-credential")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    fn trace(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("trace.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    /// Safe failure summary only. Raw trace bodies and arbitrary worker stderr
    /// deliberately remain private; assertions expose fixed stages/counts only.
    fn diagnostic(&self) -> String {
        let trace = self.trace();
        let mut stages = std::collections::BTreeMap::<&str, usize>::new();
        let mut errors = std::collections::BTreeMap::<&str, usize>::new();
        let allowed_stages = [
            "bootstrap",
            "entered",
            "guardReady",
            "protocolLoaded",
            "workerLoaded",
            "stdinAttached",
            "inputRejected",
            "networkDenied",
            "workerReturned",
            "bootstrapFailed",
        ];
        let allowed_errors = [
            "ERR_MODULE_NOT_FOUND",
            "ERR_UNSUPPORTED_ESM_URL_SCHEME",
            "EACCES",
            "ENOENT",
            "EPIPE",
            "other",
        ];
        for line in std::fs::read_to_string(self.directory.path().join("diagnostic.jsonl"))
            .unwrap_or_default()
            .lines()
        {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(stage) = allowed_stages
                .iter()
                .find(|stage| value["stage"].as_str() == Some(**stage))
            {
                *stages.entry(stage).or_default() += 1;
            }
            if let Some(error) = allowed_errors
                .iter()
                .find(|error| value["errorClass"].as_str() == Some(**error))
            {
                *errors.entry(error).or_default() += 1;
            }
        }
        json!({"start":trace.iter().filter(|v|v["kind"]=="start").count(),
            "GET":trace.iter().filter(|v|v["kind"]=="network" && v["method"]=="GET").count(),
            "POST":trace.iter().filter(|v|v["kind"]=="network" && v["method"]=="POST").count(),
            "exit":trace.iter().filter(|v|v["kind"]=="exit").count(),
            "stages":stages,"errorClasses":errors})
        .to_string()
    }
    fn assert_status(&self, response: &axum::response::Response, expected: StatusCode) {
        assert_eq!(
            response.status(),
            expected,
            "safe worker stages: {}",
            self.diagnostic()
        );
    }
    fn sent(&self) -> Vec<Value> {
        self.trace()
            .into_iter()
            .filter(|v| v["kind"] == "network" && v["method"] == "POST")
            .collect()
    }
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.proxy.state.status.read().await.active_connections != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("original ActiveConnectionGuard must release at terminal/drop");
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.listener.abort();
        for task in self.upstream.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}
fn response_tools() -> Value {
    json!([{"type":"function","name":"fixture_read","description":"Harmless client fixture only","strict":false,"parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}])
}
async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), MAX_CAPTURE)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn sse_json(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}
fn final_response(text: &str, streaming: bool) -> Value {
    if !streaming {
        return serde_json::from_str(text).unwrap();
    }
    let events = sse_json(text);
    let completed: Vec<_> = events
        .iter()
        .filter(|v| v["type"] == "response.completed")
        .collect();
    assert_eq!(completed.len(), 1, "exactly one Responses completion");
    completed[0]["response"].clone()
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_responses_two_round_tools_preserve_id_full_history_and_schema() {
    for (streaming, source) in [
        (false, "qoder_sdk"),
        (true, "qoder_sdk"),
        (false, "qoder_cn_device"),
        (true, "qoder_cn_device"),
    ] {
        let f =
            HttpFixture::new_with_source(&[("qoder-a", true)], Reply::ToolsThenText, None, source)
                .await;
        let first=f.request("/v1/responses",json!({"model":MODEL,"stream":streaming,"tools":response_tools(),"input":"Read fixture"})).await;
        assert_eq!(first.status(), StatusCode::OK);
        let value = final_response(&text(first).await, streaming);
        let call = value["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["type"] == "function_call")
            .unwrap();
        assert_eq!(call["call_id"], TOOL_ID);
        assert_eq!(call["name"], "fixture_read");
        assert_eq!(
            serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
            json!({"path":"fixture.txt"})
        );
        let actual_output = value["output"].as_array().unwrap();
        assert!(
            actual_output.iter().any(|item| item["type"] == "message"
                && item["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| part["type"] == "output_text" && part["annotations"] == json!([]))),
            "the replay must include real renderer output_text annotations"
        );
        let output = "synthetic client result";
        let mut input = vec![
            json!({"role":"user","content":"Read fixture"}),
            json!({"role":"developer","content":"middle developer"}),
            json!({"role":"assistant","content":"prior"}),
            json!({"role":"system","content":"middle system"}),
        ];
        let replay_start = input.len();
        input.extend(actual_output.iter().cloned());
        assert_eq!(
            &input[replay_start..],
            actual_output.as_slice(),
            "replay every actual output item verbatim; do not strip legal fields"
        );
        input.push(json!({"type":"function_call_output","call_id":TOOL_ID,"output":output}));
        input.push(json!({"role":"user","content":"Summarize"}));
        let second=f.request("/v1/responses",json!({"model":MODEL,"stream":streaming,"tools":response_tools(),"instructions":"leading system","input":input})).await;
        let status = second.status();
        let body = text(second).await;
        assert_eq!(status, StatusCode::OK, "streaming={streaming}: {body}");
        let value = final_response(&body, streaming);
        assert!(value.to_string().contains("tool-result-summary"));
        let sent = f.sent();
        assert_eq!(sent.len(), 2);
        let body = &sent[1]["body"];
        assert_eq!(
            body["tools"][0]["function"]["parameters"],
            response_tools()[0]["parameters"]
        );
        let messages = body["messages"].as_array().unwrap();
        assert!(
            messages
                .iter()
                .all(|message| message.get("reasoning_content").is_none()),
            "only the generated converter placeholder is removed before strict worker admission"
        );
        assert!(messages.iter().any(|v| v["tool_calls"][0]["id"] == TOOL_ID));
        assert!(messages.iter().any(|v| v["role"] == "tool"
            && v["tool_call_id"] == TOOL_ID
            && v["content"] == output));
        assert!(body["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("middle developer"));
        assert!(body["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("middle system"));
        assert!(
            body.to_string().contains("Read fixture")
                && body.to_string().contains("prior")
                && body.to_string().contains("Summarize")
        );
        let system = body["system"][0]["text"].as_str().unwrap();
        assert!(system.find("leading system").unwrap() < system.find("middle developer").unwrap());
        assert!(system.find("middle developer").unwrap() < system.find("middle system").unwrap());
        let at = |needle: &str| {
            messages
                .iter()
                .position(|m| m.to_string().contains(needle))
                .unwrap()
        };
        assert!(at("Read fixture") < at("prior"));
        assert!(at("prior") <= at(TOOL_ID));
        assert!(at(TOOL_ID) < at(output));
        assert!(at(output) < at("Summarize"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a", "qoder-a"]);
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_original_pool_selects_authorized_second_account_for_responses_and_chat() {
    for endpoint in ["/v1/responses", "/v1/chat/completions"] {
        let f =
            HttpFixture::new(&[("qoder-missing", false), ("qoder-b", true)], Reply::Plain).await;
        let body = if endpoint.ends_with("responses") {
            json!({"model":MODEL,"input":"hello"})
        } else {
            json!({"model":MODEL,"messages":[{"role":"user","content":"hello"}]})
        };
        let response = f.request(endpoint, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("ordered-history-and-auth-ok"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-b"]);
        assert!(f
            .trace()
            .iter()
            .filter(|v| v["kind"] == "start")
            .all(|v| v["account"] == "qoder-b"));
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_unknown_submission_never_replays_on_second_account() {
    for streaming in [false, true] {
        let f = HttpFixture::new(
            &[("qoder-unknown", true), ("qoder-b", true)],
            Reply::Unknown,
        )
        .await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":MODEL,"stream":streaming,"input":"hello"}),
            )
            .await;
        assert!(!response.status().is_success());
        let _ = text(response).await;
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-unknown"]);
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_partial_failure_never_completes_or_replays() {
    let f = HttpFixture::new(
        &[("qoder-partial", true), ("qoder-b", true)],
        Reply::PartialFailure,
    )
    .await;
    let response = f
        .request(
            "/v1/responses",
            json!({"model":MODEL,"stream":true,"input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut captured = Vec::new();
    let mut failed = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => captured.extend_from_slice(&bytes),
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    drop(stream);
    let bytes = String::from_utf8(captured).unwrap();
    assert!(
        failed || bytes.contains("response.failed") || bytes.contains("\"error\""),
        "must observe an explicit failure"
    );
    assert!(bytes.contains("partial-before-failure"));
    assert!(!bytes.contains("response.completed"));
    assert!(!bytes.contains("PRIVATE_UPSTREAM_ERROR"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-partial"]);
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_client_body_drop_closes_upstream_without_account_replay() {
    for read_prefix in [false, true] {
        let f = HttpFixture::new(&[("qoder-a", true), ("qoder-b", true)], Reply::Hold).await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":MODEL,"stream":true,"input":"hello"}),
            )
            .await;
        f.assert_status(&response, StatusCode::OK);
        f.upstream.wait_posts(1).await;
        assert_eq!(f.proxy.state.status.read().await.active_connections, 1);
        if read_prefix {
            let mut stream = response.into_body().into_data_stream();
            let mut seen = Vec::new();
            tokio::time::timeout(Duration::from_secs(3), async {
                while !String::from_utf8_lossy(&seen).contains("visible-prefix") {
                    seen.extend_from_slice(&stream.next().await.expect("prefix").unwrap());
                }
            })
            .await
            .unwrap();
            drop(stream);
        } else {
            drop(response);
        }
        f.upstream.wait_closed(1).await;
        f.idle().await;
        assert_eq!(
            *f.upstream.posts.lock().await,
            vec!["qoder-a"],
            "safe worker stages: {}",
            f.diagnostic()
        );
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_revocation_before_first_final_body_poll_rejects_buffered_bytes() {
    let f = HttpFixture::new(&[("qoder-a", true), ("qoder-b", true)], Reply::Hold).await;
    let response = f
        .request(
            "/v1/responses",
            json!({"model":MODEL,"stream":true,"input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    // Forwarder already primed the real upstream; do not poll final Body yet.
    f.db.delete_provider("codex", "qoder-a").unwrap();
    assert!(axum::body::to_bytes(response.into_body(), MAX_CAPTURE)
        .await
        .is_err());
    f.upstream.wait_closed(1).await;
    f.idle().await;
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_revocation_after_visible_prefix_blocks_notify_released_tail() {
    let f = HttpFixture::new(
        &[("qoder-a", true), ("qoder-b", true)],
        Reply::RevocableTail,
    )
    .await;
    let response = f
        .request(
            "/v1/responses",
            json!({"model":MODEL,"stream":true,"input":"hello"}),
        )
        .await;
    f.assert_status(&response, StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut visible = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !String::from_utf8_lossy(&visible).contains("visible-prefix") {
            visible.extend_from_slice(&stream.next().await.expect("prefix frame").unwrap());
        }
    })
    .await
    .unwrap();
    f.db.delete_provider("codex", "qoder-a").unwrap();
    f.upstream.release_tail.notify_one();
    let mut failed = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => visible.extend_from_slice(&bytes),
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(failed, "final ResponseLease must fail after revocation");
    assert!(!String::from_utf8_lossy(&visible).contains(PRIVATE_TAIL));
    assert!(!String::from_utf8_lossy(&visible).contains("response.completed"));
    drop(stream);
    f.idle().await;
    assert_eq!(
        *f.upstream.posts.lock().await,
        vec!["qoder-a"],
        "safe worker stages: {}",
        f.diagnostic()
    );
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_original_override_exclusion_keeps_compatible_account() {
    let f = HttpFixture::new(
        &[("qoder-incompatible", true), ("qoder-b", true)],
        Reply::Plain,
    )
    .await;
    let response = f
        .request("/v1/responses", json!({"model":MODEL,"input":"hello"}))
        .await;
    f.assert_status(&response, StatusCode::OK);
    let _ = text(response).await;
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-b"]);
    assert_eq!(f.proxy.state.status.read().await.failed_requests, 0);
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_invalid_model_and_unsupported_capabilities_never_launch_worker() {
    for body in [
        json!({"model":null,"input":"hello"}),
        json!({"model":"","input":"hello"}),
        json!({"model":42,"input":"hello"}),
        json!({"model":MODEL,"previous_response_id":"old","input":"hello"}),
        json!({"model":MODEL,"input":[{"type":"function_call_output","call_id":"orphan","output":"result"}]}),
        json!({"model":MODEL,"input":[{"type":"unknown_future_item","text":"must not disappear"}]}),
        json!({"model":MODEL,"input":"hello","tools":[{"type":"custom","name":"unsupported"}]}),
    ] {
        let f = HttpFixture::new(&[("qoder-a", true), ("qoder-b", true)], Reply::Plain).await;
        let response = f.request("/v1/responses", body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let _ = text(response).await;
        assert!(f.trace().is_empty());
        assert!(f.upstream.posts.lock().await.is_empty());
        assert!(f
            .proxy
            .state
            .status
            .read()
            .await
            .current_provider_id
            .is_none());
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_free_intent_remains_original_preflight_policy() {
    for endpoint in ["/v1/responses", "/v1/chat/completions"] {
        for streaming in [false, true] {
            for model in ["openrouter/free", "vendor/model:free"]
                .into_iter()
                .chain(crate::vendor::opencode::FREE_CHAT_MODELS.iter().copied())
            {
                let f =
                    HttpFixture::new(&[("qoder-a", true), ("qoder-b", true)], Reply::Plain).await;
                let body = if endpoint.ends_with("responses") {
                    json!({"model":model,"stream":streaming,"input":"hello"})
                } else {
                    json!({"model":model,"stream":streaming,"messages":[{"role":"user","content":"hello"}]})
                };
                let response = f.request(endpoint, body).await;
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                let _ = text(response).await;
                assert!(f.trace().is_empty());
                assert!(f.upstream.posts.lock().await.is_empty());
                f.idle().await;
            }
        }
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_worker_rejects_invalid_tool_policy_before_any_upstream_get_or_post() {
    for invalid in [
        json!({"tool_choice":"unknown"}),
        json!({"tools":[{"type":"custom","name":"run"}]}),
        json!({"tools":[{"type":"function","function":{"name":"read","strict":true}}]}),
        json!({"tools":[{"type":"function","function":{"name":"read"}},{"type":"function","function":{"name":"read"}}]}),
        json!({"tools":[{"type":"function","function":{"name":" "}}]}),
        json!({"parallel_tool_calls":"false"}),
        json!({"tool_choice":{"type":"function","function":{"name":"missing"}}}),
    ] {
        let f = HttpFixture::new(&[("qoder-a", true), ("qoder-b", true)], Reply::Plain).await;
        let mut body = json!({"model":MODEL,"messages":[{"role":"user","content":"hello"}]});
        body.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        let response = f.request("/v1/chat/completions", body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let _ = text(response).await;
        assert!(
            f.trace().is_empty(),
            "invalid policy must not start a worker"
        );
        assert!(f.upstream.posts.lock().await.is_empty());
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_supported_responses_image_is_forwarded_without_local_url_fetch() {
    let f = HttpFixture::new(&[("qoder-a", true)], Reply::Plain).await;
    let response=f.request("/v1/responses",json!({"model":MODEL,"input":[{"role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"https://fixture-image.invalid/image.png"}]}]})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = text(response).await;
    let trace = f.trace();
    assert_eq!(trace.iter().filter(|v| v["kind"] == "network").count(), 2);
    assert!(f.sent()[0]["body"]
        .to_string()
        .contains("https://fixture-image.invalid/image.png"));
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_free_only_provider_and_untrusted_runtime_hints_remain_preflight_rejections() {
    for body in [
        json!({"model":MODEL,"input":"hello","nodePath":"/untrusted/node"}),
        json!({"model":MODEL,"input":"hello","sdkRoot":"/untrusted/sdk"}),
        json!({"model":MODEL,"input":"hello","options":{"transport":"untrusted","auth":"ignored"}}),
    ] {
        let f = HttpFixture::new(&[("qoder-a", true)], Reply::Plain).await;
        let response = f.request("/v1/responses", body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = text(response).await;
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        let sent = f.sent()[0]["body"].to_string();
        assert!(!sent.contains("/untrusted/") && !sent.contains("\"transport\":\"untrusted\""));
        f.idle().await;
    }
    let f = HttpFixture::new(&[("qoder-a", true)], Reply::Plain).await;
    let mut provider =
        f.db.get_provider_by_id("qoder-a", "codex")
            .unwrap()
            .unwrap();
    provider.settings_config["freeOnly"] = json!(true);
    f.db.save_provider("codex", &provider).unwrap();
    for endpoint in ["/v1/responses", "/v1/chat/completions"] {
        for streaming in [false, true] {
            let body = if endpoint.ends_with("responses") {
                json!({"model":MODEL,"stream":streaming,"input":"hello"})
            } else {
                json!({"model":MODEL,"stream":streaming,"messages":[{"role":"user","content":"hello"}]})
            };
            let response = f.request(endpoint, body).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let _ = text(response).await;
        }
    }
    assert!(f.trace().is_empty());
    assert!(f.upstream.posts.lock().await.is_empty());
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_complete_chat_done_cannot_hide_missing_end_or_nonzero_worker_exit() {
    for terminal_fault in ["missing-end", "nonzero", "extra-after-done"] {
        let f = HttpFixture::new_with_terminal_fault(
            &[("qoder-a", true), ("qoder-b", true)],
            Reply::Plain,
            Some(terminal_fault),
        )
        .await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":MODEL,"stream":true,"input":"hello"}),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "ordinary prefix is allowed before terminal host failure"
        );
        let mut stream = response.into_body().into_data_stream();
        let mut visible = Vec::new();
        let mut failed = false;
        tokio::time::timeout(Duration::from_secs(4), async {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => visible.extend_from_slice(&bytes),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        drop(stream);
        let text = String::from_utf8(visible).unwrap();
        assert!(
            text.contains("ordered-history-and-auth-ok"),
            "prefix must be real upstream output"
        );
        assert!(
            !text.contains("response.completed"),
            "unverified host terminal must not be published: {terminal_fault}"
        );
        assert!(
            !text.contains("EXTRA_AFTER_DONE_MUST_NOT_ESCAPE"),
            "no data after controlled DONE may escape: {terminal_fault}"
        );
        assert!(
            failed || text.contains("response.failed") || text.contains("\"error\""),
            "host terminal failure must remain observable"
        );
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        assert_eq!(f.trace().iter().filter(|v| v["kind"] == "start").count(), 1);
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_concurrent_chat_requests_have_independent_completion_ids() {
    let f = HttpFixture::new(&[("qoder-a", true)], Reply::Plain).await;
    let request =
        json!({"model":MODEL,"stream":false,"messages":[{"role":"user","content":"hello"}]});
    let (first, second) = tokio::join!(
        f.request("/v1/chat/completions", request.clone()),
        f.request("/v1/chat/completions", request),
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let first: Value = serde_json::from_str(&text(first).await).unwrap();
    let second: Value = serde_json::from_str(&text(second).await).unwrap();
    for value in [&first, &second] {
        assert_eq!(
            value["choices"][0]["message"]["content"],
            "ordered-history-and-auth-ok"
        );
        assert!(value["id"].as_str().is_some_and(|id| !id.is_empty()));
    }
    assert_ne!(
        first["id"], second["id"],
        "concurrent requests must not share completion identity"
    );
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a", "qoder-a"]);
    assert_eq!(f.trace().iter().filter(|v| v["kind"] == "start").count(), 2);
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_fragmented_private_jsonl_still_completes_chat() {
    let f = HttpFixture::new_with_terminal_fault(
        &[("qoder-a", true)],
        Reply::Plain,
        Some("fragmented-jsonl"),
    )
    .await;
    let response = f
        .request(
            "/v1/chat/completions",
            json!({"model":MODEL,"stream":true,"messages":[{"role":"user","content":"hello"}]}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    let events = sse_json(&body);
    assert!(events
        .iter()
        .any(|event| event["choices"][0]["delta"]["content"] == "ordered-history-and-auth-ok"));
    assert_eq!(
        body.lines().filter(|line| *line == "data: [DONE]").count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["choices"][0]["finish_reason"] == "stop")
            .count(),
        1
    );
    assert!(!events.iter().any(|event| event.get("error").is_some()));
    assert!(f
        .trace()
        .iter()
        .any(|event| event["kind"] == "fragmented-jsonl"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_cleanup_failure_after_complete_chat_cannot_publish_success() {
    for endpoint in ["/v1/chat/completions", "/v1/responses"] {
        let f = HttpFixture::new_with_terminal_fault(
            &[("qoder-a", true), ("qoder-b", true)],
            Reply::Plain,
            Some("cleanup-failure"),
        )
        .await;
        let body = if endpoint.ends_with("responses") {
            json!({"model":MODEL,"stream":true,"input":"hello"})
        } else {
            json!({"model":MODEL,"stream":true,"messages":[{"role":"user","content":"hello"}]})
        };
        let response = f.request(endpoint, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let mut visible = Vec::new();
        let mut failed = false;
        tokio::time::timeout(Duration::from_secs(4), async {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => visible.extend_from_slice(&bytes),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        drop(stream);
        let body = String::from_utf8(visible).unwrap();
        assert!(body.contains("ordered-history-and-auth-ok"));
        assert!(failed || body.contains("response.failed") || body.contains("\"error\""));
        assert!(!body.contains("response.completed"));
        assert!(!body.lines().any(|line| line == "data: [DONE]"));
        assert!(
            !sse_json(&body)
                .iter()
                .any(|event| event["choices"][0]["finish_reason"] == "stop"),
            "cleanup failure must preserve the old no-success-finish assertion"
        );
        assert!(f
            .trace()
            .iter()
            .any(|event| event["kind"] == "cleanup-failure"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        assert_eq!(
            f.trace()
                .iter()
                .filter(|event| event["kind"] == "start")
                .count(),
            1
        );
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_chat_three_message_history_ignores_request_auth_and_runtime_hints() {
    for streaming in [false, true] {
        let f = HttpFixture::new(&[("qoder-a", true)], Reply::Plain).await;
        let history = json!([{"role":"user","content":"first"},{"role":"assistant","content":"prior"},{"role":"user","content":"second"}]);
        let response = f.request("/v1/chat/completions", json!({"model":MODEL,"stream":streaming,"messages":history,"tools":[],"options":{"auth":"untrusted"},"nodePath":"/untrusted/node","sdkRoot":"/untrusted/sdk"})).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = text(response).await;
        if streaming {
            assert!(sse_json(&body)
                .iter()
                .any(|event| event["choices"][0]["delta"]["content"]
                    == "ordered-history-and-auth-ok"));
            assert_eq!(
                body.lines().filter(|line| *line == "data: [DONE]").count(),
                1
            );
        } else {
            let value: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(
                value["choices"][0]["message"]["content"],
                "ordered-history-and-auth-ok"
            );
        }
        let trace = f.trace();
        let start = trace.iter().find(|event| event["kind"] == "start").unwrap();
        assert_eq!(
            start["chat"]["messages"], history,
            "the original Chat history must be transmitted unchanged"
        );
        for key in ["options", "nodePath", "sdkRoot"] {
            assert!(start["chat"].get(key).is_none());
        }
        let sent = f.sent();
        assert_eq!(sent.len(), 1);
        let wire = &sent[0]["body"];
        assert!(
            wire.get("tools").is_none() || wire["tools"] == json!([]),
            "no implicit tool declaration"
        );
        let messages: Vec<_> = wire["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] != "system")
            .collect();
        assert_eq!(messages.len(), 3);
        for (message, (role, expected)) in messages.iter().zip([
            ("user", "first"),
            ("assistant", "prior"),
            ("user", "second"),
        ]) {
            assert_eq!(message["role"], role);
            let content = if let Some(text) = message["content"].as_str() {
                text.to_owned()
            } else {
                message["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|part| part["text"].as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("")
            };
            assert_eq!(content, expected);
        }
        assert!(!wire.to_string().contains("untrusted"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_nonstream_source_error_is_bad_gateway_without_completion_or_replay() {
    for endpoint in ["/v1/chat/completions", "/v1/responses"] {
        let f = HttpFixture::new(
            &[("qoder-a", true), ("qoder-b", true)],
            Reply::PartialFailure,
        )
        .await;
        let request = if endpoint.ends_with("responses") {
            json!({"model":MODEL,"stream":false,"input":"hello"})
        } else {
            json!({"model":MODEL,"stream":false,"messages":[{"role":"user","content":"hello"}]})
        };
        let response = f.request(endpoint, request).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_GATEWAY,
            "the fixed nonstream source-error regression must retain exact 502"
        );
        let body = text(response).await;
        let value: Value = serde_json::from_str(&body).unwrap();
        assert!(value.get("error").is_some());
        assert!(value.get("choices").is_none());
        assert!(value.get("output").is_none());
        assert!(!body.contains("PRIVATE_UPSTREAM_ERROR"));
        assert!(
            !body.contains("partial-before-failure"),
            "nonstream partial success must remain buffered on failure"
        );
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        assert_eq!(
            f.trace()
                .iter()
                .filter(|event| event["kind"] == "start")
                .count(),
            1
        );
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_cn_device_original_pool_selects_ready_account_without_pat_or_auth_exchange() {
    for endpoint in ["/v1/responses", "/v1/chat/completions"] {
        let f = HttpFixture::new_with_source(
            &[("qoder-missing", false), ("qoder-b", true)],
            Reply::Plain,
            None,
            "qoder_cn_device",
        )
        .await;
        let body = if endpoint.ends_with("responses") {
            json!({"model":MODEL,"input":"hello"})
        } else {
            json!({"model":MODEL,"messages":[{"role":"user","content":"hello"}]})
        };
        let response = f.request(endpoint, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("ordered-history-and-auth-ok"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-b"]);
        let sent = f.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["origin"], "https://gateway.qoder.com.cn");
        assert_eq!(sent[0]["credentialKind"], "china_qoder_device");
        assert_eq!(sent[0]["body"]["session_type"], "qodercli");
        assert_eq!(sent[0]["body"]["business"]["product"], "cli");
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_cn_device_unknown_post_and_partial_output_never_replay_or_complete() {
    for reply in [Reply::Unknown, Reply::PartialFailure] {
        let f = HttpFixture::new_with_source(
            &[("qoder-a", true), ("qoder-b", true)],
            reply,
            None,
            "qoder_cn_device",
        )
        .await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":MODEL,"stream":true,"input":"hello"}),
            )
            .await;
        let mut stream = response.into_body().into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(_) => break,
            }
        }
        assert!(!String::from_utf8_lossy(&bytes).contains("response.completed"));
        assert_eq!(
            *f.upstream.posts.lock().await,
            vec!["qoder-a"],
            "safe worker stages: {}",
            f.diagnostic()
        );
        f.idle().await;
    }
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_cn_device_auth_refusal_retires_exact_owner_without_replaying_second_account() {
    let f = HttpFixture::new_with_source(
        &[("qoder-a", true), ("qoder-b", true)],
        Reply::AuthRejected,
        None,
        "qoder_cn_device",
    )
    .await;
    let response = f
        .request("/v1/responses", json!({"model":MODEL,"input":"hello"}))
        .await;
    assert!(!response.status().is_success());
    assert!(text(response).await.contains("reauthorization_required"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
    let now = chrono::Utc::now().timestamp_millis();
    for (id, expected) in [("qoder-a", "unknown"), ("qoder-b", "ready")] {
        let context = f.db.qoder_http_identity_context(id, now).unwrap();
        assert_eq!(serde_json::to_value(context).unwrap()["status"], expected);
    }
    f.idle().await;
}

#[cfg(test)]
#[tokio::test]
#[serial_test::serial]
async fn http_cn_device_business_403_keeps_ready_credentials_and_never_replays() {
    let f = HttpFixture::new_with_source(
        &[("qoder-a", true), ("qoder-b", true)],
        Reply::BusinessRejected,
        None,
        "qoder_cn_device",
    )
    .await;
    let response = f
        .request("/v1/responses", json!({"model":MODEL,"input":"hello"}))
        .await;
    assert!(!response.status().is_success());
    assert!(!text(response).await.contains("reauthorization_required"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
    for id in ["qoder-a", "qoder-b"] {
        let context =
            f.db.qoder_http_identity_context(id, chrono::Utc::now().timestamp_millis())
                .unwrap();
        assert_eq!(serde_json::to_value(context).unwrap()["status"], "ready");
    }
    f.idle().await;
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub(crate) async fn verify_shared(case: &str) {
    if matches!(
        case,
        "mixed"
            | "mixed_review"
            | "bad_mm"
            | "bad_wb"
            | "bad_qoder"
            | "pending_auto"
            | "pending_manual"
            | "pending_new_group"
            | "bad_qoder_runtime"
            | "unsupported_manual"
            | "unknown_member"
            | "conflicting_member_model"
            | "all_unsupported"
    ) {
        return verify_mixed(case).await;
    }
    use crate::services::{routing_groups as groups, shared_routing as routing};
    let source = if case == "cn" {
        "qoder_cn_device"
    } else {
        "qoder_sdk"
    };
    let f = HttpFixture::new_with_source(
        &[("qoder-a", true), ("qoder-b", true)],
        match case {
            "tools" => Reply::ToolsThenText,
            "submitted_unknown" => Reply::Unknown,
            "submitted_partial" => Reply::PartialFailure,
            _ => Reply::Plain,
        },
        None,
        source,
    )
    .await;
    for id in ["qoder-a", "qoder-b"] {
        let mut p = f.db.get_provider_by_id(id, "codex").unwrap().unwrap();
        let config = p.settings_config["config"].as_str().unwrap();
        p.settings_config["config"] = json!(format!("model = {MODEL:?}\n{config}"));
        if case == "model_options" {
            p.meta
                .get_or_insert_with(Default::default)
                .local_proxy_request_overrides =
                Some(crate::provider::LocalProxyRequestOverrides {
                    body: Some(json!({"model":"synthetic-forbidden-late-qoder-model"})),
                    ..Default::default()
                });
        }
        f.db.save_provider("codex", &p).unwrap();
    }
    crate::settings::unlock_settings_for_test(f.db.secrets.clone()).unwrap();
    let state = crate::store::AppState::new(f.db.clone()).unwrap();
    let sources = crate::commands::get_service_sources_impl(&state).unwrap();
    let mut members = Vec::new();
    for id in ["qoder-a", "qoder-b"] {
        let row = sources
            .iter()
            .find(|s| s.bindings.iter().any(|b| b.provider_id == id));
        assert!(row.is_some(), "original authorized Qoder account must project an explicit configured-model shared source");
        let row = row.unwrap();
        assert_eq!(row.bindings.len(), 1);
        assert_eq!(row.bindings[0].app_id, "codex");
        assert_eq!(row.bindings[0].model.as_deref(), Some(MODEL));
        let dto = serde_json::to_string(row).unwrap();
        for private in [
            "synthetic-http-uid",
            "synthetic-http-access",
            "synthetic-original-pat",
            "machineId",
            "stableEvidence",
        ] {
            assert!(!dto.contains(private));
        }
        members.push(groups::Member {
            source_id: row.source_id.clone(),
            model: MODEL.into(),
        });
    }
    if case == "source" {
        return;
    }
    if matches!(
        case,
        "review" | "tools" | "group_epoch" | "target_epoch" | "model_options"
    ) {
        Database::apply_upstream4_migrations_on_conn(&f.db.conn.lock().unwrap()).unwrap();
        {
            let vault = f.db.secret_session().read().unwrap();
            crate::mode::state::update_app(
                &crate::live::engine::DeviceStore::for_device(),
                &vault,
                "codex",
                |entry| {
                    entry.mode = Some(crate::mode::state::Mode::Direct);
                    Ok(())
                },
            )
            .unwrap();
        }
        let prepared = tokio::task::block_in_place(|| {
            routing::prepare_group(
                &state,
                routing::GroupDraft {
                    id: None,
                    expected_revision: None,
                    name: "Qoder shared".into(),
                    members: members.clone(),
                },
            )
        })
        .unwrap();
        if case == "group_epoch" {
            let session = f.db.secrets.clone();
            let result = tokio::task::block_in_place(|| {
                crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
                    if point == "shared_group:validated" {
                        session.set_blocked(true);
                        session.set_blocked(false);
                    }
                })));
                let result = routing::confirm_group(&state, prepared.clone());
                crate::mode::operation::failpoint::on_boundary(None);
                result
            });
            assert!(
                result.is_err(),
                "Qoder-only group CAS must reject block/unblock during this operation"
            );
            assert!(groups::read(&f.db.conn.lock().unwrap(), &prepared.group.id)
                .unwrap()
                .is_none());
            tokio::task::block_in_place(|| routing::confirm_group(&state, prepared)).unwrap();
            return;
        }
        let group =
            tokio::task::block_in_place(|| routing::confirm_group(&state, prepared)).unwrap();
        let app = crate::app_config::AppType::Codex;
        let bootstrap = Provider::with_id(
            "synthetic-bootstrap".into(),
            "Bootstrap".into(),
            crate::relay::provider_config::settings_config_for(
                &app,
                "synthetic-bootstrap-key",
                "Fixture",
                "http://127.0.0.1:9",
                "bootstrap-model",
            )
            .unwrap(),
            None,
        );
        f.db.save_provider("codex", &bootstrap).unwrap();
        tokio::task::block_in_place(|| {
            crate::services::ProviderService::switch(&state, app.clone(), &bootstrap.id)
        })
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app("codex", true)
            .await
            .unwrap();
        let target = groups::RouteTarget::OrderedGroup { group_id: group.id };
        let prepared =
            tokio::task::block_in_place(|| routing::prepare_target(&state, &app, target)).unwrap();
        if case == "target_epoch" {
            let session = f.db.secrets.clone();
            let result = tokio::task::block_in_place(|| {
                crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
                    if point == "route_target:validated" {
                        session.set_blocked(true);
                        session.set_blocked(false);
                    }
                })));
                let result = routing::apply_target(&state, &app, prepared.clone());
                crate::mode::operation::failpoint::on_boundary(None);
                result
            });
            assert!(
                result.is_err(),
                "Qoder-only target CAS must reject block/unblock during this operation"
            );
            assert!(groups::binding(&f.db.conn.lock().unwrap(), "codex")
                .unwrap()
                .is_none());
            let recovered = tokio::task::block_in_place(|| {
                routing::recover_target(&state, &app, prepared.request.clone())
            })
            .unwrap();
            assert_eq!(
                recovered.status, "completed",
                "persisted intent must recover under the new operation epoch"
            );
            state.proxy_service.stop().await.unwrap();
            return;
        }
        let result =
            tokio::task::block_in_place(|| routing::apply_target(&state, &app, prepared)).unwrap();
        assert_eq!(result.status, "completed");
    } else {
        let mut conn = f.db.conn.lock().unwrap();
        let group = groups::save(&mut conn, "qoder-shared", None, &members).unwrap();
        let target = if case == "manual" {
            groups::RouteTarget::Manual {
                source_id: members[0].source_id.clone(),
                model: MODEL.into(),
            }
        } else {
            groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            }
        };
        groups::set_target(
            &mut conn,
            "codex",
            target,
            None,
            (case != "manual").then_some(group.revision),
        )
        .unwrap();
    }
    if matches!(case, "submitted_unknown" | "submitted_partial") {
        let response = f
            .request(
                "/v1/responses",
                json!({"model":"caller-alias","stream":true,"input":"hello"}),
            )
            .await;
        if case == "submitted_unknown" {
            assert!(!response.status().is_success());
            let _ = text(response).await;
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let mut stream = response.into_body().into_data_stream();
            let mut captured = Vec::new();
            let mut failed = false;
            tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => captured.extend_from_slice(&bytes),
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                }
            })
            .await
            .unwrap();
            drop(stream);
            let body = String::from_utf8(captured).unwrap();
            assert!(failed || body.contains("response.failed") || body.contains("\"error\""));
            assert!(body.contains("partial-before-failure"));
            assert!(!body.contains("response.completed"));
            assert!(!body.contains("PRIVATE_UPSTREAM_ERROR"));
        }
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
        verify_shared_trace(&f, 1);
        f.idle().await;
        return;
    }
    if case == "tools" {
        let first = f
            .request(
                "/v1/responses",
                json!({"model":"caller-alias","tools":response_tools(),"input":"Read fixture"}),
            )
            .await;
        assert_eq!(first.status(), StatusCode::OK);
        let value = final_response(&text(first).await, false);
        let output = value["output"].as_array().unwrap();
        let call = output
            .iter()
            .find(|v| v["type"] == "function_call")
            .unwrap();
        assert_eq!(call["call_id"], TOOL_ID);
        let mut input = vec![json!({"role":"user","content":"Read fixture"})];
        input.extend(output.iter().cloned());
        input.push(json!({"type":"function_call_output","call_id":TOOL_ID,"output":"synthetic client result"}));
        let second = f
            .request(
                "/v1/responses",
                json!({"model":"caller-alias","tools":response_tools(),"input":input}),
            )
            .await;
        assert_eq!(second.status(), StatusCode::OK);
        assert!(text(second).await.contains("tool-result-summary"));
        let sent = f.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1]["body"]["tools"][0]["function"]["parameters"],
            response_tools()[0]["parameters"]
        );
        assert!(sent[1]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["role"] == "tool"
                && v["tool_call_id"] == TOOL_ID
                && v["content"] == "synthetic client result"));
        assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a", "qoder-a"]);
        verify_shared_trace(&f, 2);
        state.proxy_service.stop().await.unwrap();
        f.idle().await;
        return;
    }
    let response = f
        .request(
            "/v1/responses",
            json!({"model":"caller-alias", "input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("ordered-history-and-auth-ok"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
    verify_shared_trace(&f, 1);
    if matches!(case, "review" | "model_options") {
        state.proxy_service.stop().await.unwrap();
    }
    f.idle().await;
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
fn verify_shared_trace(f: &HttpFixture, requests: usize) {
    let trace = f.trace();
    let network = trace
        .iter()
        .filter(|v| v["kind"] == "network")
        .collect::<Vec<_>>();
    assert_eq!(network.len(), requests * 2);
    for pair in network.chunks_exact(2) {
        assert_eq!(pair[0]["method"], "GET");
        assert_eq!(pair[1]["method"], "POST");
        assert_eq!(pair[0]["origin"], pair[1]["origin"]);
        assert!(matches!(
            pair[0]["origin"].as_str(),
            Some("https://api3.qoder.sh" | "https://gateway.qoder.com.cn")
        ));
        assert_eq!(pair[0]["cosyPresent"], true);
        assert_eq!(pair[1]["cosyPresent"], true);
        assert_eq!(pair[1]["body"]["model_config"]["key"], MODEL);
    }
    assert!(trace
        .iter()
        .filter(|v| v["kind"] == "start")
        .all(|v| v["chat"]["model"] == MODEL));
}
#[cfg(all(feature = "gui", feature = "test-hooks"))]
async fn verify_mixed(case: &str) {
    use crate::{
        app_config::AppType,
        services::{routing_groups as groups, shared_routing as routing},
    };
    let f = HttpFixture::new_with_refresh(
        &[("qoder-a", true)],
        Reply::Plain,
        None,
        "qoder_sdk",
        case.starts_with("pending_"),
    )
    .await;
    let mut q =
        f.db.get_provider_by_id("qoder-a", "codex")
            .unwrap()
            .unwrap();
    q.settings_config["config"] = json!(format!(
        "model = {MODEL:?}\n{}",
        q.settings_config["config"].as_str().unwrap()
    ));
    f.db.save_provider("codex", &q).unwrap();
    let platform = if case == "bad_wb" {
        "workbuddy"
    } else {
        "minimax_code"
    };
    let model = if platform == "workbuddy" {
        "synthetic-wb-model"
    } else {
        "MiniMax-M2.7"
    };
    let account = "synthetic-mixed-mm";
    let owner = if platform == "minimax_code" {
        Some(
            crate::minimax_code::inference::FixtureTransport::seed_accounts(
                f.db.secret_session(),
                &[(account, crate::minimax_code::catalog::Region::Global)],
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    for app in [AppType::Claude, AppType::Codex] {
        let config = if matches!(app, AppType::Claude) {
            json!({"env":{"ANTHROPIC_MODEL":model}})
        } else {
            json!({"auth":{},"config":format!("model = {model:?}\n")})
        };
        let mut p = Provider::with_id(
            format!("mixed-{}", app.as_str()),
            "Mixed native".into(),
            config,
            None,
        );
        p.category = Some("custom".into());
        p.meta=Some(serde_json::from_value(json!({"authBinding":{"source":"managed_account","authProvider":platform,"accountId":account},"apiFormat":if platform=="workbuddy"{"openai_chat"}else{"anthropic"},"commonConfigEnabled":false})).unwrap());
        crate::services::provider::managed_inference::validate(&app, &p).unwrap();
        f.db.save_provider(app.as_str(), &p).unwrap();
        if matches!(app, AppType::Claude) {
            f.db.set_current_provider("claude", &p.id).unwrap();
        }
    }
    crate::settings::unlock_settings_for_test(f.db.secrets.clone()).unwrap();
    let state = crate::store::AppState::new(f.db.clone()).unwrap();
    if case == "bad_qoder" {
        f.db.conn.lock().unwrap().execute("UPDATE providers SET qoder_http_secret='synthetic malformed envelope' WHERE id='qoder-a' AND app_type='codex'",[]).unwrap();
        let rows = crate::commands::get_service_sources_impl(&state)
            .expect("bad Qoder row must not hide verified MiniMax sources");
        assert!(rows
            .iter()
            .any(|s| s.bindings.iter().any(|b| b.provider_id == "mixed-codex")));
        assert!(rows
            .iter()
            .all(|s| s.bindings.iter().all(|b| b.provider_id != "qoder-a")));
        let view = crate::commands::application_overview(&state, &AppType::Codex).unwrap();
        let bad = view
            .configurations
            .iter()
            .find(|c| c.provider_id == "qoder-a")
            .unwrap();
        assert!(!bad.can_select);
        assert!(bad.account.is_none());
        assert_eq!(
            serde_json::to_value(&bad.presentation).unwrap()["switchBlockedReason"],
            "nativeSourceUnavailable"
        );
        return;
    }
    if matches!(case, "bad_mm" | "bad_wb") {
        let file = if platform == "workbuddy" {
            crate::secrets::owned_file::WORKBUDDY_FILE
        } else {
            crate::minimax_code::catalog::FILE
        };
        std::fs::write(
            f.db.secret_session().root().join(file),
            b"synthetic corrupt owned document",
        )
        .unwrap();
        let rows = crate::commands::get_service_sources_impl(&state)
            .expect("unrelated bad owner must not hide verified Qoder source");
        assert_eq!(
            rows.iter()
                .filter(|s| s.bindings.iter().any(|b| b.provider_id == "qoder-a"))
                .count(),
            1
        );
        assert!(rows.iter().all(|s| s
            .bindings
            .iter()
            .all(|b| !b.provider_id.starts_with("mixed-"))));
        let overview = crate::commands::application_overview(&state, &AppType::Codex).unwrap();
        let bad = overview
            .configurations
            .iter()
            .find(|c| c.provider_id == "mixed-codex")
            .unwrap();
        assert!(!bad.can_select);
        assert!(bad.account.is_none());
        assert_eq!(
            serde_json::to_value(&bad.presentation).unwrap()["switchBlockedReason"],
            "nativeSourceUnavailable"
        );
        return;
    }
    let rows = crate::commands::get_service_sources_impl(&state).unwrap();
    let q = rows
        .iter()
        .find(|s| s.bindings.iter().any(|b| b.provider_id == "qoder-a"))
        .unwrap();
    let mm = rows
        .iter()
        .find(|s| s.bindings.iter().any(|b| b.provider_id == "mixed-codex"))
        .unwrap();
    let mut members = vec![
        groups::Member {
            source_id: q.source_id.clone(),
            model: MODEL.into(),
        },
        groups::Member {
            source_id: mm.source_id.clone(),
            model: model.into(),
        },
    ];
    let invalid_capability = matches!(
        case,
        "unsupported_manual" | "unknown_member" | "conflicting_member_model" | "all_unsupported"
    );
    match case {
        "bad_qoder_runtime" => {
            members.remove(0);
        }
        "unknown_member" => {
            members[0].source_id = "native:qoder_sdk:unknown-synthetic-source".into();
        }
        "conflicting_member_model" => {
            members[0].model = "different-configured-model".into();
        }
        "all_unsupported" => {
            members.truncate(1);
        }
        _ => {}
    }
    let group = if invalid_capability {
        // Seed previously bound intent to exercise runtime safety, not publication success.
        groups::save(
            &mut f.db.conn.lock().unwrap(),
            "qoder-capability-negative",
            None,
            &members,
        )
        .unwrap()
    } else {
        let prepared = tokio::task::block_in_place(|| {
            routing::prepare_group(
                &state,
                routing::GroupDraft {
                    id: None,
                    expected_revision: None,
                    name: "One mixed group".into(),
                    members: members.clone(),
                },
            )
        })
        .unwrap();
        tokio::task::block_in_place(|| routing::confirm_group(&state, prepared)).unwrap()
    };
    for app in ["claude", "codex"] {
        groups::set_target(
            &mut f.db.conn.lock().unwrap(),
            app,
            groups::RouteTarget::OrderedGroup {
                group_id: group.id.clone(),
            },
            None,
            Some(group.revision),
        )
        .unwrap();
    }
    if case == "bad_qoder_runtime" {
        f.db.conn.lock().unwrap().execute("UPDATE providers SET qoder_http_secret='synthetic malformed envelope' WHERE id='qoder-a' AND app_type='codex'", []).unwrap();
    }
    if case == "unsupported_manual" {
        groups::set_target(
            &mut f.db.conn.lock().unwrap(),
            "claude",
            groups::RouteTarget::Manual {
                source_id: members[0].source_id.clone(),
                model: MODEL.into(),
            },
            Some(1),
            None,
        )
        .unwrap();
    }
    if case.starts_with("pending_") {
        let provider =
            f.db.get_provider_by_id("qoder-a", "codex")
                .unwrap()
                .unwrap();
        let source =
            crate::services::provider::qoder_http_auth::SourceBinding::from_provider(&provider)
                .unwrap()
                .0;
        assert!(matches!(
            f.db.qoder_http_acquire(&source, chrono::Utc::now().timestamp_millis() + 3_600_000)
                .unwrap(),
            crate::services::provider::qoder_http_auth::Acquisition::Refresh(_)
        ));
        if case == "pending_new_group" {
            let result = tokio::task::block_in_place(|| {
                routing::prepare_group(
                    &state,
                    routing::GroupDraft {
                        id: None,
                        expected_revision: None,
                        name: "Unavailable new group".into(),
                        members: vec![members[0].clone()],
                    },
                )
            });
            assert!(result.is_err(), "UnsupportedApp is identity evidence, not a ready supported projection for a new group");
            assert!(f.trace().is_empty());
            assert!(f.upstream.posts.lock().await.is_empty());
            return;
        }
        if case == "pending_manual" {
            groups::set_target(
                &mut f.db.conn.lock().unwrap(),
                "codex",
                groups::RouteTarget::Manual {
                    source_id: members[0].source_id.clone(),
                    model: MODEL.into(),
                },
                Some(1),
                None,
            )
            .unwrap();
            let response = f
                .request(
                    "/v1/responses",
                    json!({"model":"caller-alias","input":"hello"}),
                )
                .await;
            assert!(!response.status().is_success());
            let _ = text(response).await;
            assert!(f.upstream.posts.lock().await.is_empty());
            return;
        }
    }
    if case == "mixed_review" {
        let update = tokio::task::block_in_place(|| {
            routing::prepare_group(
                &state,
                routing::GroupDraft {
                    id: Some(group.id),
                    expected_revision: Some(group.revision),
                    name: "Same mixed group".into(),
                    members,
                },
            )
        })
        .expect("one bound global group must keep valid per-app capabilities");
        tokio::task::block_in_place(|| routing::confirm_group(&state, update)).unwrap();
        return;
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let upstream=axum::Router::new().route("/global/mavis/api/v1/llm/v1/messages",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<Value>|{let calls=captured.clone();async move{calls.fetch_add(1,Ordering::SeqCst);assert_eq!(body["model"],"MiniMax-M2.7");assert_eq!(headers["authorization"],"Bearer synthetic-mixed-mm-access");axum::Json(json!({"id":"synthetic-mm","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":"mixed-native-ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = crate::minimax_code::inference::FixtureTransport::loopback(
        owner.as_ref().unwrap(),
        &format!("http://{}", listener.local_addr().unwrap()),
    )
    .await
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let response=f.proxy.build_router().layer(axum::Extension(transport.clone())).oneshot(axum::http::Request::builder().method("POST").uri("/v1/messages").header("content-type","application/json").body(axum::body::Body::from(json!({"model":"caller-alias","max_tokens":32,"messages":[{"role":"user","content":"hello"}]}).to_string())).unwrap()).await.unwrap();
    if invalid_capability {
        assert!(
            !response.status().is_success(),
            "unsupported manual/unknown/model-conflict/empty app capability must not route"
        );
        let _ = text(response).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "must not skip unknown/conflicting intent and send to MiniMax"
        );
        assert!(f.upstream.posts.lock().await.is_empty());
        assert!(
            f.trace().is_empty(),
            "invalid intent must not launch Qoder worker"
        );
        server.abort();
        return;
    }
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("mixed-native-ok"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(f.upstream.posts.lock().await.is_empty());
    if matches!(case, "pending_auto" | "bad_qoder_runtime") {
        let response = f
            .proxy
            .build_router()
            .layer(axum::Extension(transport))
            .layer(axum::Extension(f.runtime.clone()))
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        json!({"model":"caller-alias","input":"hello"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("mixed-native-ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(f.upstream.posts.lock().await.is_empty());
        assert!(
            f.trace().is_empty(),
            "unrelated/pending Qoder owner must not launch its worker"
        );
        server.abort();
        return;
    }
    let response = f
        .request(
            "/v1/responses",
            json!({"model":"caller-alias","input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("ordered-history-and-auth-ok"));
    assert_eq!(*f.upstream.posts.lock().await, vec!["qoder-a"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let conn = f.db.conn.lock().unwrap();
    assert_eq!(
        groups::binding(&conn, "claude").unwrap().unwrap().target,
        groups::binding(&conn, "codex").unwrap().unwrap().target
    );
    drop(conn);
    server.abort();
}
