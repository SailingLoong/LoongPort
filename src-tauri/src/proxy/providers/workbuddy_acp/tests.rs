use super::*;
use axum::{extract::State, response::IntoResponse, routing::get, Json};
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub(crate) struct Mock {
    sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<String>>>>,
    pub(crate) connections: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) methods: Arc<Mutex<Vec<String>>>,
    stop: &'static str,
}
fn update(text: &str) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-a","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})
}
async fn receive(State(state): State<Mock>, headers: http::HeaderMap) -> axum::response::Response {
    state
        .connections
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(headers["authorization"], "Bearer SYNTHETIC_TICKET");
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    *state.sender.lock().await = Some(sender);
    let stream = futures::stream::unfold(receiver, |mut receiver| async {
        receiver
            .recv()
            .await
            .map(|value| (Ok::<_, std::io::Error>(value), receiver))
    });
    http::Response::builder()
        .header("content-type", "text/event-stream")
        .header("Acp-Connection-Id", "connection-a")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}
async fn send(
    State(state): State<Mock>,
    headers: http::HeaderMap,
    Json(rpc): Json<Value>,
) -> impl IntoResponse {
    assert_eq!(headers["authorization"], "Bearer SYNTHETIC_TICKET");
    assert_eq!(headers["acp-connection-id"], "connection-a");
    let method = rpc["method"].as_str().unwrap().to_owned();
    state.methods.lock().await.push(method.clone());
    let sender = state.sender.lock().await.clone().unwrap();
    if method == "session/load" {
        sender
            .send(format!("data: {}\r\n\r\n", update("OLD HISTORY")))
            .await
            .unwrap();
    }
    if method == "session/prompt" && state.stop == "permission" {
        sender
            .send(format!(
                "data: {}\n\n",
                json!({"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{}})
            ))
            .await
            .unwrap();
        return http::StatusCode::ACCEPTED;
    }
    if method == "session/prompt" {
        sender
            .send(format!("data: {}\n\n", update("hello")))
            .await
            .unwrap();
    }
    if method == "session/prompt" && matches!(state.stop, "disconnect" | "slow") {
        if state.stop == "disconnect" {
            *state.sender.lock().await = None;
        }
        return http::StatusCode::ACCEPTED;
    }
    let result = if method == "session/prompt" {
        json!({"stopReason":state.stop,"usage":{}})
    } else if method == "initialize" {
        json!({"protocolVersion":1,"agentCapabilities":{"loadSession":state.stop!="no_load"}})
    } else {
        json!({})
    };
    sender
        .send(format!(
            "data: {}\n\n",
            json!({"jsonrpc":"2.0","id":rpc["id"],"result":result})
        ))
        .await
        .unwrap();
    http::StatusCode::ACCEPTED
}
fn provider(link: String) -> Provider {
    let mut row = Provider::with_id(
        "wb-a".into(),
        "Synthetic ACP".into(),
        json!({"workbuddy_acp":{"account_id":"account-a","session_id":"session-a","link":link,"gateway_ticket":"SYNTHETIC_TICKET","declared_model":"configured-label"}}),
        None,
    );
    row.meta = Some(crate::provider::ProviderMeta {
        auth_binding: Some(crate::provider::AuthBinding {
            source: crate::provider::AuthBindingSource::ManagedAccount,
            auth_provider: Some("workbuddy".into()),
            account_id: Some("account-a".into()),
        }),
        ..Default::default()
    });
    row
}
pub(crate) fn request(stream: bool) -> Value {
    json!({"model":"configured-label","stream":stream,"messages":[{"role":"user","content":"hi"}]})
}
pub(crate) async fn server(stop: &'static str) -> (Provider, Mock, tokio::task::JoinHandle<()>) {
    let state = Mock {
        sender: Arc::new(Mutex::new(None)),
        connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        methods: Arc::new(Mutex::new(vec![])),
        stop,
    };
    let app = axum::Router::new()
        .route("/session", get(receive).post(send))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut row = provider(format!(
        "http://127.0.0.1:{}/session",
        listener.local_addr().unwrap().port()
    ));
    let owner = format!("synthetic-{}", uuid::Uuid::new_v4());
    row.settings_config["workbuddy_acp"]["account_id"] = json!(owner);
    row.meta
        .as_mut()
        .unwrap()
        .auth_binding
        .as_mut()
        .unwrap()
        .account_id = Some(owner);
    let task = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
    (row, state, task)
}
#[tokio::test]
async fn workbuddy_acp_text_actual_http_stream_discards_load_replay() {
    let (row, state, task) = server("end_turn").await;
    let response = forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .await
    .unwrap();
    let bytes = response.bytes_with_limit(1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("hello"));
    assert!(!text.contains("OLD HISTORY"));
    assert_eq!(text.matches("message_stop").count(), 2); // event name plus JSON type
    assert!(!text.contains("input_tokens"));
    let events: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert!(crate::proxy::usage::parser::TokenUsage::from_claude_stream_events(&events).is_none());
    assert_eq!(
        *state.methods.lock().await,
        vec!["initialize", "session/load", "session/prompt"]
    );
    task.abort();
}
#[tokio::test]
async fn workbuddy_acp_text_refusal_never_emits_success_stop() {
    let (row, _, task) = server("refusal").await;
    let response = forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .await
    .unwrap();
    let mut stream = response.bytes_stream();
    let mut output = String::new();
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => output.push_str(&String::from_utf8_lossy(&bytes)),
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(failed);
    assert!(!output.contains("message_stop"));
    task.abort();
}
#[tokio::test]
async fn workbuddy_acp_text_rejects_unbound_account_and_unsupported_model() {
    let mut row = provider("http://127.0.0.1:1/session".into());
    row.meta
        .as_mut()
        .unwrap()
        .auth_binding
        .as_mut()
        .unwrap()
        .account_id = Some("other".into());
    assert!(forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true)
    )
    .await
    .is_err());
    let row = provider("http://127.0.0.1:1/session".into());
    let mut body = request(true);
    body["model"] = json!("other");
    assert!(forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &body
    )
    .await
    .is_err());
}

#[tokio::test]
async fn workbuddy_acp_text_rejects_missing_load_capability_before_loading() {
    let (row, state, task) = server("no_load").await;
    assert!(forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true)
    )
    .await
    .is_err());
    assert_eq!(*state.methods.lock().await, vec!["initialize"]);
    task.abort();
}
#[tokio::test]
async fn workbuddy_acp_text_rejects_sampling_tools_and_history_before_network() {
    let row = provider("http://127.0.0.1:1/session".into());
    for (key, value) in [
        ("max_tokens", json!(16)),
        ("temperature", json!(0.5)),
        ("tools", json!([])),
        ("system", json!("unsupported")),
    ] {
        let mut body = request(true);
        body[key] = value;
        assert!(input(
            &AppType::Claude,
            &http::Method::POST,
            &row,
            "/v1/messages",
            &body
        )
        .is_err());
    }
    let mut body = request(true);
    body["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"history"}));
    assert!(input(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &body
    )
    .is_err());
}
#[tokio::test]
async fn workbuddy_acp_text_active_session_guard_releases_on_drop_and_isolates_accounts() {
    let row = provider("http://127.0.0.1:1/session".into());
    let (mut config, _, _) = input(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .unwrap();
    let first = session_guard(&config).unwrap();
    assert!(session_guard(&config).is_err());
    config.account_id = "other-account".into();
    let other = session_guard(&config).unwrap();
    config.account_id = "account-a".into();
    drop(first);
    assert!(session_guard(&config).is_ok());
    drop(other);
}
#[tokio::test]
async fn workbuddy_acp_text_permission_request_is_terminal_without_reply() {
    let (row, state, task) = server("permission").await;
    let response = forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .await
    .unwrap();
    assert!(response.bytes_with_limit(1024 * 1024).await.is_err());
    assert_eq!(
        *state.methods.lock().await,
        vec!["initialize", "session/load", "session/prompt"]
    );
    task.abort();
}

#[tokio::test]
async fn workbuddy_acp_text_nonstream_uses_existing_conversion_and_unique_ids() {
    let (row, _, task) = server("end_turn").await;
    let mut ids = vec![];
    for _ in 0..2 {
        let response = forward(
            &AppType::Claude,
            &http::Method::POST,
            &row,
            "/v1/messages",
            &request(false),
        )
        .await
        .unwrap();
        let bytes = response.bytes_with_limit(1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["content"][0]["text"], "hello");
        assert!(value.get("usage").is_none());
        ids.push(value["id"].clone());
    }
    assert_ne!(ids[0], ids[1]);
    task.abort();
}
#[tokio::test]
async fn workbuddy_acp_text_drop_closes_http_and_releases_account_session() {
    let (row, state, task) = server("slow").await;
    let response = forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .await
    .unwrap();
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !text.contains("hello") {
            text.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
    })
    .await
    .unwrap();
    assert!(forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true)
    )
    .await
    .is_err());
    let sender = state.sender.lock().await.clone().unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), sender.closed())
        .await
        .unwrap();
    let (config, _, _) = input(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .unwrap();
    assert!(session_guard(&config).is_ok());
    task.abort();
}
#[tokio::test]
async fn workbuddy_acp_text_sse_utf8_multiline_and_identified_replay() {
    let data = json!({"jsonrpc":"2.0","method":"session/update","params":{"text":"合成"}});
    let frame="id: event-a\r\ndata: {\r\ndata: \"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"text\":\"合成\"}}\r\n\r\n".to_owned();
    let input = format!("{frame}{frame}data: {data}\n\ndata: {data}\n\n");
    let chunks = input
        .as_bytes()
        .chunks(1)
        .map(|chunk| Ok::<_, reqwest::Error>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let mut events = Events {
        stream: Box::pin(futures::stream::iter(chunks)),
        buffer: String::new(),
        remainder: vec![],
        seen_ids: HashSet::new(),
    };
    for _ in 0..3 {
        assert_eq!(events.next().await.unwrap(), data);
    }
    assert!(events.next().await.is_err());
}

#[tokio::test]
async fn workbuddy_acp_text_rejects_oversized_event_id_without_echoing_it() {
    let marker = "synthetic-private-id".repeat(100);
    let frame = format!("id: {marker}\ndata: {{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\n\n");
    let mut events = Events {
        stream: Box::pin(futures::stream::iter([Ok::<_, reqwest::Error>(
            Bytes::from(frame),
        )])),
        buffer: String::new(),
        remainder: vec![],
        seen_ids: HashSet::new(),
    };
    let error = events.next().await.unwrap_err().to_string();
    assert!(!error.contains("synthetic-private-id"));
    assert!(events.seen_ids.is_empty());
}
#[tokio::test]
async fn workbuddy_acp_text_cancelled_round_and_ticket_url_are_not_accepted() {
    let (row, _, task) = server("cancelled").await;
    let response = forward(
        &AppType::Claude,
        &http::Method::POST,
        &row,
        "/v1/messages",
        &request(true),
    )
    .await
    .unwrap();
    assert!(response.bytes_with_limit(1024 * 1024).await.is_err());
    task.abort();
    for url in [
        "https://user:secret@example.invalid/session",
        "https://example.invalid/session?ticket=SYNTHETIC_PRIVATE",
    ] {
        let row = provider(url.into());
        let error = input(
            &AppType::Claude,
            &http::Method::POST,
            &row,
            "/v1/messages",
            &request(true),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(!error.contains("SYNTHETIC_PRIVATE"));
        assert!(!error.contains("secret"));
    }
}
