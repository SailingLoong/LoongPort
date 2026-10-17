//! Chat Completions text-only dispatch for an already selected Qoder account.
use super::qoder_host::{HostConfig, QoderHost};
use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};

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
    if body.get("tools").is_some()
        || body.get("tool_choice").is_some()
        || body.get("previous_response_id").is_some()
    {
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
        loop {
            let seconds=if first{dispatch.first_byte_timeout}else{dispatch.idle_timeout};
            let next=if seconds==0{host.next_event().await}else{match tokio::time::timeout(Duration::from_secs(seconds),host.next_event()).await{Ok(r)=>r,Err(_)=>{let _=host.cancel().await;yield sse(json!({"error":{"code":"qoder_timeout"}}));break;}}};
            match next {
                Ok(Some(event))=>{
                    first=false;
                    match event["type"].as_str(){
                        Some("text_delta"|"text_snapshot")=>{
                            let key=event["messageId"].as_str().unwrap_or("").to_owned();let old=texts.entry(key).or_default();let text=event["text"].as_str().unwrap_or("");
                            let delta=if event["type"]=="text_snapshot" {match text.strip_prefix(old.as_str()){Some(t)=>t.to_owned(),None=>{yield sse(json!({"error":{"code":"qoder_text_conflict"}}));break;}}}else{text.to_owned()};
                            old.push_str(&delta);if !delta.is_empty(){yield sse(chunk(&stream_id,&stream_model,Some(&delta),false));}
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
        b["tools"] = json!([]);
        assert_eq!(
            request("does-not-exist", b).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
}
