use std::{
    sync::{Arc, Mutex, Once},
    time::Duration,
};

use axum::{
    extract::{OriginalUri, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use log::{Log, Metadata, Record};
use serde_json::{json, Value};

use crate::{
    app_config::AppType,
    database::Database,
    provider::{Provider, ProviderMeta},
    relay::model_verification::{
        active::BalancedActiveVerifier,
        coordinator::{ActiveVerifier, ModelVerificationCoordinator, VerificationEventSink},
        types::{RunFailureKind, TargetKey, TargetScope, VerificationProgressEvent},
    },
};

const PROVIDER_ID: &str = "loongport-0123456789abcdef";
const URL_SENTINEL: &str = "SENTINEL_PRIVACY_URL_7B9E2A";
const API_KEY_SENTINEL: &str = "SENTINEL_PRIVACY_API_KEY_31D4C8";
const ASSISTANT_SENTINEL: &str = "SENTINEL_PRIVACY_ASSISTANT_946AF0";
const THINKING_SENTINEL: &str = "SENTINEL_PRIVACY_THINKING_8ED315";
const SIGNATURE_SENTINEL: &str = "SENTINEL_PRIVACY_SIGNATURE_F625BC";
const TOOL_ARGUMENTS_SENTINEL: &str = "SENTINEL_PRIVACY_TOOL_ARGUMENTS_0CA479";

const CODEX_PROMPTS: &[&str] = &[
    "Reply with ready.",
    "Call report_probe with ready set to true.",
    "Return the fixed verification object.",
    "Reply with stream.",
];
const CLAUDE_PROMPTS: &[&str] = &[
    "Reply with the word ready.",
    "Call report_probe with an object containing ready: true.",
    "Reply with the word stream.",
    "Think briefly, then reply ready.",
    "Continue and reply ready.",
];

#[derive(Clone, Copy)]
enum MockProtocol {
    Codex,
    Claude,
}

#[derive(Clone)]
struct MockState {
    protocol: MockProtocol,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

#[derive(Debug)]
struct CapturedRequest {
    path: String,
    authorization: Option<String>,
    api_key: Option<String>,
    body: Value,
}

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<VerificationProgressEvent>>,
    changed: Mutex<Vec<TargetScope>>,
}

impl VerificationEventSink for RecordingSink {
    fn emit_progress(&self, event: &VerificationProgressEvent) -> Result<(), ()> {
        self.progress.lock().unwrap().push(event.clone());
        Err(())
    }

    fn emit_changed(&self, scope: &TargetScope) -> Result<(), ()> {
        self.changed.lock().unwrap().push(scope.clone());
        Err(())
    }
}

struct CapturingLogger {
    records: Mutex<Vec<String>>,
}

impl Log for CapturingLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.records.lock().unwrap().push(format!(
                "{} {} {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

static LOGGER: CapturingLogger = CapturingLogger {
    records: Mutex::new(Vec::new()),
};
static LOGGER_INIT: Once = Once::new();

#[tokio::test]
async fn active_protocols_keep_private_request_and_response_material_out_of_every_sink() {
    init_logger();
    LOGGER.records.lock().unwrap().clear();

    for (protocol, app_type, model, prompts) in [
        (
            MockProtocol::Codex,
            AppType::Codex,
            "gpt-5.6-sol",
            CODEX_PROMPTS,
        ),
        (
            MockProtocol::Claude,
            AppType::Claude,
            "claude-sonnet-5",
            CLAUDE_PROMPTS,
        ),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (endpoint, server) = spawn_mock(protocol, requests.clone()).await;
        let db = managed_db(&endpoint, app_type.clone());
        let sink = Arc::new(RecordingSink::default());
        let verifier = Arc::new(BalancedActiveVerifier::new(db.clone()));
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            db.clone(),
            verifier,
            sink.clone(),
        ));
        let target = TargetKey::new(PROVIDER_ID, app_type.as_str(), model);

        let start = coordinator.start(target).await.unwrap();
        wait_for_change(&sink).await;
        let reports = coordinator
            .list_results(&[PROVIDER_ID.to_string()])
            .unwrap();

        let captured = requests.lock().unwrap();
        let captured_debug = format!("{captured:?}");
        assert!(captured_debug.contains(URL_SENTINEL));
        assert!(captured_debug.contains(API_KEY_SENTINEL));
        for prompt in prompts {
            assert!(
                captured_debug.contains(prompt),
                "mock did not observe request prompt {prompt:?}"
            );
        }
        drop(captured);

        let returned = serde_json::to_string(&(start, &reports)).unwrap();
        assert_no_private_values("returned start/report", &returned, prompts);

        let persisted = persisted_row(&db);
        assert_no_private_values("SQLite result row", &persisted, prompts);

        let emitted = serde_json::to_string(&(
            sink.progress.lock().unwrap().clone(),
            sink.changed.lock().unwrap().clone(),
        ))
        .unwrap();
        assert_no_private_values("progress/change events", &emitted, prompts);

        assert_eq!(reports.len(), 1);
        server.abort();
    }

    let logged = LOGGER.records.lock().unwrap().join("\n");
    assert!(logged.contains("模型验证进度事件发送失败"));
    assert!(logged.contains("模型验证结果变化事件发送失败"));
    assert_no_private_values(
        "captured log output",
        &logged,
        &[CODEX_PROMPTS, CLAUDE_PROMPTS].concat(),
    );
}

#[tokio::test]
async fn unsupported_apps_and_user_providers_are_rejected_before_network_io() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (endpoint, server) = spawn_mock(MockProtocol::Codex, requests.clone()).await;
    let db = managed_db(&endpoint, AppType::Codex);
    db.save_provider(
        "codex",
        &Provider {
            id: "user-provider".to_string(),
            name: "User provider".to_string(),
            settings_config: json!({"auth":{"OPENAI_API_KEY":API_KEY_SENTINEL}}),
            website_url: Some(endpoint),
            category: Some("aggregator".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: Some(ProviderMeta {
                loongport_account_id: Some(7),
                ..Default::default()
            }),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    )
    .unwrap();
    let verifier = BalancedActiveVerifier::new(db);

    assert!(matches!(
        verifier.prepare(TargetKey::new(PROVIDER_ID, "gemini", "gemini-2.5")),
        Err(RunFailureKind::InvalidResponse)
    ));
    assert!(matches!(
        verifier.prepare(TargetKey::new("user-provider", "codex", "gpt-5.6-sol")),
        Err(RunFailureKind::InvalidResponse)
    ));
    tokio::task::yield_now().await;
    assert!(requests.lock().unwrap().is_empty());
    server.abort();
}

fn init_logger() {
    LOGGER_INIT.call_once(|| {
        log::set_logger(&LOGGER).expect("test logger should be the first installed logger");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

async fn spawn_mock(
    protocol: MockProtocol,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let route = format!("/{URL_SENTINEL}/v1/*rest");
    let app = Router::new()
        .route(&route, post(mock_handler))
        .with_state(MockState { protocol, requests });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/{URL_SENTINEL}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (endpoint, server)
}

async fn mock_handler(
    State(state): State<MockState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.requests.lock().unwrap().push(CapturedRequest {
        path: uri.path().to_string(),
        authorization: header_value(&headers, header::AUTHORIZATION.as_str()),
        api_key: header_value(&headers, "x-api-key"),
        body: body.clone(),
    });

    match state.protocol {
        MockProtocol::Codex => codex_response(&body),
        MockProtocol::Claude => claude_response(&body),
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn private_echo(request: &Value) -> Value {
    json!({
        "request": request,
        "url": URL_SENTINEL,
        "apiKey": API_KEY_SENTINEL,
        "assistant": ASSISTANT_SENTINEL,
        "thinking": THINKING_SENTINEL,
        "signature": SIGNATURE_SENTINEL,
        "toolArguments": TOOL_ARGUMENTS_SENTINEL,
    })
}

fn codex_response(body: &Value) -> Response {
    let model = body["model"].as_str().unwrap();
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let events = [
            json!({"type":"response.created","response":{"object":"response","model":model,"private":private_echo(body)}}),
            json!({"type":"response.output_item.added","item":{"type":"message"}}),
            json!({"type":"response.content_part.added","part":{"type":"output_text"}}),
            json!({"type":"response.output_text.delta","delta":ASSISTANT_SENTINEL}),
            json!({"type":"response.content_part.done"}),
            json!({"type":"response.output_item.done"}),
            json!({"type":"response.completed","response":{"object":"response","status":"completed","model":model,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5},"private":private_echo(body)}}),
        ];
        return sse_response(&events);
    }

    let output = if body.get("tools").is_some() {
        json!([{
            "type":"function_call",
            "name":"report_probe",
            "arguments":TOOL_ARGUMENTS_SENTINEL,
        }])
    } else if body.get("text").is_some() {
        json!([{
            "type":"message",
            "content":[{"type":"output_text","text":"{\"ready\":true}"}],
        }])
    } else {
        json!([{
            "type":"message",
            "content":[{"type":"output_text","text":ASSISTANT_SENTINEL}],
        }])
    };
    Json(json!({
        "object":"response",
        "status":"completed",
        "model":model,
        "output":output,
        "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5},
        "private":private_echo(body),
    }))
    .into_response()
}

fn claude_response(body: &Value) -> Response {
    let model = body["model"].as_str().unwrap();
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let events = [
            json!({"type":"message_start","message":{"model":model,"usage":{"input_tokens":3,"output_tokens":0},"private":private_echo(body)}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":ASSISTANT_SENTINEL}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ];
        return sse_response(&events);
    }

    if body.get("thinking").is_some() {
        return Json(json!({
            "type":"message",
            "model":model,
            "content":[{
                "type":"thinking",
                "thinking":THINKING_SENTINEL,
                "signature":SIGNATURE_SENTINEL,
            }],
            "usage":{"input_tokens":3,"output_tokens":2},
            "private":private_echo(body),
        }))
        .into_response();
    }

    let is_continuation = body["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|message| message["role"] == "assistant")
    });
    let content = if body.get("tools").is_some() {
        json!([{
            "type":"tool_use",
            "name":"report_probe",
            "input":{"private":TOOL_ARGUMENTS_SENTINEL},
        }])
    } else {
        json!([{"type":"text","text":ASSISTANT_SENTINEL}])
    };
    Json(json!({
        "type":"message",
        "model":model,
        "content":content,
        "usage":{"input_tokens":3,"output_tokens":2},
        "private":private_echo(body),
        "continuation":is_continuation,
    }))
    .into_response()
}

fn sse_response(events: &[Value]) -> Response {
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

fn managed_db(endpoint: &str, app_type: AppType) -> Arc<Database> {
    let db = Database::memory().unwrap();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO loongport_relay (site_origin, site_name, api_base_url, account_id, account_label, login_identifier, auth_token, sort_index) \
             VALUES (?1, 'Privacy test', ?1, 7, 'privacy', 'privacy', 'token', 0)",
            [endpoint],
        )
        .unwrap();
    }
    let settings_config = match app_type {
        AppType::Codex => json!({"auth":{"OPENAI_API_KEY":API_KEY_SENTINEL}}),
        AppType::Claude => json!({"env":{"ANTHROPIC_AUTH_TOKEN":API_KEY_SENTINEL}}),
        _ => unreachable!("privacy coverage only exercises active protocols"),
    };
    db.save_provider(
        app_type.as_str(),
        &Provider {
            id: PROVIDER_ID.to_string(),
            name: "Privacy test tier".to_string(),
            settings_config,
            website_url: Some(endpoint.to_string()),
            category: Some("aggregator".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: Some(ProviderMeta {
                loongport_account_id: Some(7),
                ..Default::default()
            }),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    )
    .unwrap();
    Arc::new(db)
}

async fn wait_for_change(sink: &RecordingSink) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !sink.changed.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("active verification should finish");
}

fn persisted_row(db: &Database) -> String {
    let conn = db.conn.lock().unwrap();
    conn.query_row(
        "SELECT provider_id, app_type, model, active_report_json, passive_aggregate_json, verdict, evidence_level, rules_version, active_checked_at, passive_observed_at, updated_at, notified_fingerprints_json FROM model_verification_results",
        [],
        |row| {
            (0..12)
                .map(|index| row.get_ref(index).map(|value| format!("{value:?}")))
                .collect::<Result<Vec<_>, _>>()
                .map(|values| values.join("|"))
        },
    )
    .unwrap()
}

fn assert_no_private_values(label: &str, material: &str, prompts: &[&str]) {
    for sentinel in [
        URL_SENTINEL,
        API_KEY_SENTINEL,
        ASSISTANT_SENTINEL,
        THINKING_SENTINEL,
        SIGNATURE_SENTINEL,
        TOOL_ARGUMENTS_SENTINEL,
    ]
    .into_iter()
    .chain(prompts.iter().copied())
    {
        assert!(
            !material.contains(sentinel),
            "{label} leaked private sentinel {sentinel:?}"
        );
    }
}
