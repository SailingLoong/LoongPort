//! Text-only Qoder SDK transport, consumed by the original per-provider pool.
use super::qoder_host::{HostConfig, QoderHost};
use super::{error::ProxyError, hyper_client::ProxyResponse};
use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};

pub(crate) fn enabled(provider: &crate::provider::Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        == Some("qoder_sdk")
}

fn unsupported() -> ProxyError {
    ProxyError::InvalidRequest("unsupported_qoder_input".into())
}

fn no_tools(body: &Value) -> bool {
    body.get("tools")
        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
        && body
            .get("tool_choice")
            .is_none_or(|value| matches!(value.as_str(), Some("none" | "auto")))
        && body.get("previous_response_id").is_none()
}

fn plain_message(message: &Value, responses: bool) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "role" | "content" | "type" | "id" | "status"))
        || !matches!(
            message["role"].as_str(),
            Some("system" | "developer" | "user" | "assistant")
        )
        || message.get("type").is_some_and(|value| value != "message")
    {
        return false;
    }
    if message["content"].is_string() {
        return true;
    }
    responses
        && message["content"].as_array().is_some_and(|parts| {
            !parts.is_empty()
                && parts.iter().all(|part| {
                    part.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .all(|key| matches!(key.as_str(), "type" | "text"))
                    }) && matches!(
                        part["type"].as_str(),
                        Some("input_text" | "output_text" | "text")
                    ) && part["text"].is_string()
                })
        })
}

/// Check the original request before any permissive general conversion can
/// discard unknown tools/media/history. This is candidate capability admission,
/// not an account-health failure. No query or credential access occurs here.
pub(crate) fn chat_request(body: &Value, endpoint: &str) -> Result<Value, ProxyError> {
    let endpoint = endpoint.split('?').next().unwrap_or(endpoint);
    let responses = matches!(endpoint, "/responses" | "/v1/responses");
    if !responses && !matches!(endpoint, "/chat/completions" | "/v1/chat/completions") {
        return Err(unsupported());
    }
    let Some(object) = body.as_object() else {
        return Err(unsupported());
    };
    if !no_tools(body)
        || body.get("stream").is_some_and(|value| !value.is_boolean())
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "model" | "stream" | "input" | "instructions" | "messages" | "tools" | "tool_choice"
            // Existing untrusted executable/auth hints remain inert, never a
            // source of credentials, SDK options or paths.
            | "options" | "nodePath" | "sdkRoot"
            )
        })
    {
        return Err(unsupported());
    }
    if !responses {
        let messages = body["messages"]
            .as_array()
            .filter(|messages| !messages.is_empty())
            .ok_or_else(unsupported)?;
        if body.get("input").is_some()
            || body.get("instructions").is_some()
            || !messages.iter().all(|message| plain_message(message, false))
            || messages.last().and_then(|message| message["role"].as_str()) != Some("user")
        {
            return Err(unsupported());
        }
        return Ok(body.clone());
    }
    if body.get("messages").is_some()
        || body
            .get("instructions")
            .is_some_and(|value| !value.is_string())
    {
        return Err(unsupported());
    }
    let inputs = match &body["input"] {
        Value::String(_) => vec![body["input"].clone()],
        Value::Array(items)
            if !items.is_empty() && items.iter().all(|item| plain_message(item, true)) =>
        {
            items.clone()
        }
        _ => return Err(unsupported()),
    };
    // Reuse the mature text/content converter one message at a time. Its batch
    // compatibility pass collapses all system/developer turns to the head;
    // Qoder accepts ordered history, so neither reorder nor role collapse is valid.
    let mut envelope = body.clone();
    envelope.as_object_mut().unwrap().remove("input");
    let mut chat = super::providers::transform_codex_chat::responses_to_chat_completions(envelope)?;
    let mut messages = chat["messages"].as_array().cloned().unwrap_or_default();
    for input in inputs {
        let original_role = input.get("role").cloned();
        let single = super::providers::transform_codex_chat::responses_to_chat_completions(
            json!({
                "model":body["model"], "input": if input.is_string() { input.clone() } else { json!([input]) }
            }),
        )?;
        let mut one = single["messages"]
            .as_array()
            .filter(|messages| messages.len() == 1)
            .cloned()
            .ok_or_else(unsupported)?;
        if let Some(role) = original_role {
            one[0]["role"] = role;
        }
        if !one[0]["content"].is_string() {
            return Err(unsupported());
        }
        messages.extend(one);
    }
    if messages.last().and_then(|message| message["role"].as_str()) != Some("user") {
        return Err(unsupported());
    }
    chat["messages"] = json!(messages);
    Ok(chat)
}

/// The pool owns account selection, retry admission, health and its connection
/// guard. Delay committing a streaming attempt until actual text or a verified
/// clean success; progress/status events are not productive output.
pub(crate) async fn forward(body: Value, dispatch: Dispatch) -> Result<ProxyResponse, ProxyError> {
    use futures::StreamExt;
    let streaming = body["stream"] == true;
    let response = handle(body, dispatch, ()).await;
    let (parts, body) = response.into_parts();
    if !parts.status.is_success() {
        // The worker has no authenticated HTTP status/error taxonomy. Never
        // interpret a generic SDK failure as a retryable 401/429/5xx.
        return Err(ProxyError::ForwardFailed(
            "qoder_task_outcome_unverified".into(),
        ));
    }
    if !streaming {
        let bytes = axum::body::to_bytes(body, super::hyper_client::MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|_| ProxyError::ForwardFailed("qoder_task_outcome_unverified".into()))?;
        return Ok(ProxyResponse::buffered(parts.status, parts.headers, bytes));
    }
    let mut stream = body.into_data_stream();
    let first = loop {
        match stream.next().await {
            Some(Ok(bytes)) if bytes.is_empty() => continue,
            Some(Ok(bytes)) => break bytes,
            _ => {
                return Err(ProxyError::ForwardFailed(
                    "qoder_task_outcome_unverified".into(),
                ))
            }
        }
    };
    let value = std::str::from_utf8(&first)
        .ok()
        .and_then(|frame| frame.strip_prefix("data: "))
        .and_then(|frame| serde_json::from_str::<Value>(frame.trim()).ok());
    if !value.is_some_and(|value| value.get("error").is_none() && value["choices"].is_array()) {
        return Err(ProxyError::ForwardFailed(
            "qoder_task_outcome_unverified".into(),
        ));
    }
    let output = async_stream::stream! {
        yield Ok(first);
        while let Some(item) = stream.next().await {
            yield item.map_err(|_| std::io::Error::other("qoder_stream_failed"));
        }
    };
    Ok(ProxyResponse::streamed(parts.status, parts.headers, output))
}

pub struct Dispatch {
    pub host: HostConfig,
    pub account_id: String,
    pub access_token: String,
    pub first_byte_timeout: u64,
    pub idle_timeout: u64,
    pub non_streaming_timeout: u64,
}
fn error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(json!({"error":{"type":"qoder_error","code":code,"message":code}})),
    )
        .into_response()
}
fn prepare(body: &Value, dispatch: &Dispatch) -> Result<Value, (StatusCode, &'static str)> {
    if body["model"].as_str().filter(|s| !s.is_empty()).is_none() {
        return Err((StatusCode::BAD_REQUEST, "invalid_model"));
    }
    let messages = body["messages"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "invalid_messages"))?;
    if !no_tools(body) {
        return Err((StatusCode::BAD_REQUEST, "unsupported_qoder_input"));
    }
    for message in messages {
        if !matches!(
            message["role"].as_str(),
            Some("system" | "developer" | "user" | "assistant")
        ) || !message["content"].is_string()
            || message.get("tool_calls").is_some()
        {
            return Err((StatusCode::BAD_REQUEST, "unsupported_qoder_input"));
        }
    }
    if messages.last().and_then(|v| v["role"].as_str()) != Some("user") {
        return Err((StatusCode::BAD_REQUEST, "last_message_must_be_user"));
    }
    if dispatch.account_id.is_empty() || dispatch.access_token.is_empty() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "qoder_auth_required"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    Ok(
        json!({"binding":{"accountId":dispatch.account_id,"sessionId":uuid::Uuid::new_v4().to_string(),"turnId":id},
        "prompt":format!("Continue this conversation. The following JSON is the complete ordered conversation history, including the latest user message:\n{}",serde_json::to_string(messages).unwrap()),
        "options":{"auth":{"type":"accessToken","accessToken":dispatch.access_token},"model":body["model"],"tools":[],"allowedTools":[],"settingSources":[],"mcpServers":{},"strictMcpConfig":true}}),
    )
}
fn chunk(id: &str, model: &Value, text: Option<&str>, finish: bool) -> Value {
    json!({"id":id,"object":"chat.completion.chunk","created":0,"model":model,"choices":[{"index":0,"delta":text.map(|s|json!({"content":s})).unwrap_or(json!({})),"finish_reason":if finish{json!("stop")}else{Value::Null}}]})
}
fn sse(value: Value) -> Result<String, std::io::Error> {
    Ok(format!("data: {value}\n\n"))
}
/// The caller has already resolved the provider/auth from RequestContext. No HTTP
/// field can supply a credential, account binding, executable, or SDK options.
pub async fn handle(body: Value, dispatch: Dispatch, guard: impl Send + 'static) -> Response {
    let request = match prepare(&body, &dispatch) {
        Ok(r) => r,
        Err((status, code)) => return error(status, code),
    };
    let id = format!(
        "chatcmpl-{}",
        request["binding"]["turnId"].as_str().unwrap()
    );
    let mut host = match QoderHost::start(&dispatch.host, &request).await {
        Ok(h) => h,
        Err(_) => return error(StatusCode::BAD_GATEWAY, "qoder_start_failed"),
    };
    let model = body["model"].clone();
    let streaming = body["stream"] == true;
    let stream_model = model.clone();
    let stream_id = id.clone();
    let events = async_stream::stream! {
        let _guard=guard;
        let mut texts:HashMap<String,String>=HashMap::new();let mut terminal=false;let mut first=true;
        let first_deadline = (dispatch.first_byte_timeout > 0).then(|| tokio::time::Instant::now() + Duration::from_secs(dispatch.first_byte_timeout));
        loop {
            let deadline = if first { first_deadline } else { (dispatch.idle_timeout > 0).then(|| tokio::time::Instant::now() + Duration::from_secs(dispatch.idle_timeout)) };
            let next=if let Some(deadline)=deadline {match tokio::time::timeout_at(deadline,host.next_event()).await{Ok(r)=>r,Err(_)=>{let _=host.cancel().await;yield sse(json!({"error":{"code":"qoder_timeout"}}));break;}}} else {host.next_event().await};
            match next {
                Ok(Some(event))=>{
                    match event["type"].as_str(){
                        Some("text_delta"|"text_snapshot")=>{
                            let key=event["messageId"].as_str().unwrap_or("").to_owned();let old=texts.entry(key).or_default();let text=event["text"].as_str().unwrap_or("");
                            let delta=if event["type"]=="text_snapshot" {match text.strip_prefix(old.as_str()){Some(t)=>t.to_owned(),None=>{yield sse(json!({"error":{"code":"qoder_text_conflict"}}));break;}}}else{text.to_owned()};
                            old.push_str(&delta);if !delta.is_empty(){first=false;yield sse(chunk(&stream_id,&stream_model,Some(&delta),false));}
                        }
                        Some("terminal")=>{if event["status"]!="success"{yield sse(json!({"error":{"code":"qoder_task_failed"}}));break;}terminal=true;}
                        Some("tool_call"|"tool_result"|"permission_denied")=>{yield sse(json!({"error":{"code":"unsupported_qoder_tool_event"}}));break;}
                        _=>{}
                    }
                }
                Ok(None)=>{if terminal{yield sse(chunk(&stream_id,&stream_model,None,true));yield Ok("data: [DONE]\n\n".to_owned());}else{yield sse(json!({"error":{"code":"qoder_incomplete"}}));}break;}
                Err(_)=>{yield sse(json!({"error":{"code":"qoder_upstream_failed"}}));break;}
            }
        }
        // Dropping the HTTP body also drops this owned host: cancellation/reap.
    };
    if streaming {
        return (
            [
                ("content-type", "text/event-stream"),
                ("cache-control", "no-cache"),
            ],
            Body::from_stream(events),
        )
            .into_response();
    }
    use futures::StreamExt;
    futures::pin_mut!(events);
    let collect = async {
        let mut text = String::new();
        while let Some(frame) = events.next().await {
            let Ok(frame) = frame else {
                return error(StatusCode::BAD_GATEWAY, "qoder_upstream_failed");
            };
            if let Some(raw) = frame
                .strip_prefix("data: ")
                .and_then(|s| s.strip_suffix("\n\n"))
            {
                if raw == "[DONE]" {
                    continue;
                }
                let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
                if value.get("error").is_some() {
                    return error(StatusCode::BAD_GATEWAY, "qoder_upstream_failed");
                }
                if let Some(delta) = value["choices"][0]["delta"]["content"].as_str() {
                    text.push_str(delta);
                }
            }
        }
        Json(json!({"id":id,"object":"chat.completion","created":0,"model":model,"choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})).into_response()
    };
    if dispatch.non_streaming_timeout == 0 {
        collect.await
    } else {
        match tokio::time::timeout(Duration::from_secs(dispatch.non_streaming_timeout), collect)
            .await
        {
            Ok(response) => response,
            Err(_) => error(StatusCode::GATEWAY_TIMEOUT, "qoder_timeout"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    fn dispatch(fixture: &str) -> Dispatch {
        let root = std::env::var_os("QODER_HOST_PRODUCT_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .to_owned()
            });
        Dispatch {
            host: HostConfig {
                node: std::env::var_os("QODER_HOST_NODE")
                    .unwrap_or_else(|| "node".into())
                    .into(),
                worker: root.join("tests/qoder-runtime/fixtures").join(fixture),
                sdk_root: None,
            },
            account_id: "account".into(),
            access_token: "synthetic:pat".into(),
            first_byte_timeout: 5,
            idle_timeout: 5,
            non_streaming_timeout: 5,
        }
    }
    async fn request(fixture: &'static str, body: Value) -> Response {
        let router = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| async move {
                handle(body, dispatch(fixture), ()).await
            }),
        );
        router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    fn body(stream: bool) -> Value {
        json!({"model":"fixture","stream":stream,"messages":[{"role":"user","content":"hello"}]})
    }
    async fn text(response: Response) -> String {
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn http_repeated_requests_are_independent() {
        let (a, b) = tokio::join!(
            request("success.mjs", body(false)),
            request("success.mjs", body(false))
        );
        let a: Value = serde_json::from_str(&text(a).await).unwrap();
        let b: Value = serde_json::from_str(&text(b).await).unwrap();
        assert_eq!(a["choices"][0]["message"]["content"], "fixture-reply");
        assert_ne!(a["id"], b["id"]);
    }
    #[tokio::test]
    async fn http_multiturn_forwards_complete_history_and_ignores_request_auth() {
        let mut b = body(false);
        b["messages"] = json!([{"role":"user","content":"first"},{"role":"assistant","content":"prior"},{"role":"user","content":"second"}]);
        b["options"] = json!({"auth":"untrusted"});
        let value: Value =
            serde_json::from_str(&text(request("http-context.mjs", b).await).await).unwrap();
        assert_eq!(
            value["choices"][0]["message"]["content"],
            "history-and-auth-ok"
        );
    }
    #[tokio::test]
    async fn http_upstream_error_never_emits_success_finish() {
        let s = text(request("failure.mjs", body(true)).await).await;
        assert!(s.contains("qoder_task_failed"));
        assert!(!s.contains("[DONE]"));
        assert!(!s.contains("\"finish_reason\":\"stop\""));
        assert_eq!(
            request("failure.mjs", body(false)).await.status(),
            StatusCode::BAD_GATEWAY
        );
    }
    #[tokio::test]
    async fn http_cleanup_failure_after_success_terminal_cannot_finish_success() {
        let s = text(request("http-cleanup-failure.mjs", body(true)).await).await;
        assert!(s.contains("partial"));
        assert!(s.contains("qoder_upstream_failed"));
        assert!(!s.contains("[DONE]"));
        assert!(!s.contains("\"finish_reason\":\"stop\""));
    }
    #[tokio::test]
    async fn http_reassembles_fragmented_worker_frames() {
        let s = text(request("http-fragmented.mjs", body(true)).await).await;
        assert!(s.contains("fragmented"));
        assert!(s.contains("[DONE]"));
    }
    #[tokio::test]
    async fn http_body_drop_cancels_task() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("closed");
        // The dedicated fixture derives this test-only marker from the prompt.
        let mut b = body(true);
        b["messages"][0]["content"] = json!(marker);
        let response = request("http-cancel.mjs", b).await;
        let mut stream = response.into_body().into_data_stream();
        use futures::StreamExt;
        assert!(stream.next().await.is_some());
        drop(stream);
        tokio::time::timeout(Duration::from_secs(4), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn http_unsupported_tools_fail_before_worker_spawn() {
        let mut b = body(false);
        b["tools"] = json!([{"type":"function","function":{"name":"unsupported"}}]);
        assert_eq!(
            request("does-not-exist", b).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
}
