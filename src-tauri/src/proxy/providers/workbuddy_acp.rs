//! Explicit account/session-bound ACP text bridge, not a generic Anthropic adapter.
//! declared_model is a local route label, never evidence of the remote model.
//! Usage stays unknown; no remote cancel method is assumed or sent.
use crate::{
    app_config::AppType,
    provider::Provider,
    proxy::{
        error::ProxyError,
        hyper_client::ProxyResponse,
        sse::{append_utf8_safe, strip_sse_field, take_sse_block},
    },
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

type SessionLock = tokio::sync::Mutex<()>;
type ActiveSessions = Mutex<HashMap<(String, String), Weak<SessionLock>>>;
static ACTIVE: OnceLock<ActiveSessions> = OnceLock::new();
const FRAME_LIMIT: usize = 1024 * 1024;
fn failure() -> ProxyError {
    ProxyError::InvalidRequest("WorkBuddy ACP text request unavailable or unsupported".into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    account_id: String,
    session_id: String,
    link: String,
    gateway_ticket: String,
    declared_model: String,
}
impl Drop for Config {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.gateway_ticket.zeroize();
    }
}
pub(crate) fn enabled(provider: &Provider) -> bool {
    provider.settings_config.get("workbuddy_acp").is_some()
}
fn input(
    app: &AppType,
    method: &http::Method,
    provider: &Provider,
    endpoint: &str,
    body: &Value,
) -> Result<(Config, String, bool), ProxyError> {
    if *app != AppType::Claude
        || *method != http::Method::POST
        || !matches!(endpoint, "/messages" | "/v1/messages")
    {
        return Err(failure());
    }
    let config: Config = serde_json::from_value(
        provider
            .settings_config
            .get("workbuddy_acp")
            .cloned()
            .ok_or_else(failure)?,
    )
    .map_err(|_| failure())?;
    if config.account_id.is_empty()
        || config.session_id.is_empty()
        || config.gateway_ticket.is_empty()
        || config.declared_model.is_empty()
        || provider
            .meta
            .as_ref()
            .and_then(|m| m.managed_account_id_for("workbuddy"))
            .as_deref()
            != Some(config.account_id.as_str())
    {
        return Err(failure());
    }
    let url = reqwest::Url::parse(&config.link).map_err(|_| failure())?;
    let secure = url.scheme() == "https";
    #[cfg(any(test, feature = "test-hooks"))]
    let secure = secure || (url.scheme() == "http" && url.host_str() == Some("127.0.0.1"));
    if !secure
        || crate::proxy::http_client::url_has_credentials(&config.link)
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(failure());
    }
    let object = body.as_object().ok_or_else(failure)?;
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "model" | "stream" | "messages"))
        || body["model"].as_str() != Some(config.declared_model.as_str())
    {
        return Err(failure());
    }
    let streaming = match body.get("stream") {
        None => false,
        Some(Value::Bool(v)) => *v,
        _ => return Err(failure()),
    };
    let messages = body["messages"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or_else(failure)?;
    let message = messages[0].as_object().ok_or_else(failure)?;
    if message.len() != 2 || messages[0]["role"] != "user" {
        return Err(failure());
    }
    let text = messages[0]["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
        .ok_or_else(failure)?
        .to_owned();
    Ok((config, text, streaming))
}
fn session_guard(config: &Config) -> Result<tokio::sync::OwnedMutexGuard<()>, ProxyError> {
    let mut sessions = ACTIVE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| failure())?;
    sessions.retain(|_, v| v.strong_count() > 0);
    let key = (config.account_id.clone(), config.session_id.clone());
    let lock = sessions
        .get(&key)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let lock = Arc::new(SessionLock::new(()));
            sessions.insert(key, Arc::downgrade(&lock));
            lock
        });
    lock.try_lock_owned().map_err(|_| failure())
}
struct Events {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    remainder: Vec<u8>,
    seen_ids: HashSet<String>,
}
impl Events {
    async fn next(&mut self) -> Result<Value, ProxyError> {
        loop {
            while let Some(block) = take_sse_block(&mut self.buffer) {
                let data = block
                    .lines()
                    .filter_map(|line| strip_sse_field(line, "data"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                if let Some(id) = block.lines().find_map(|line| strip_sse_field(line, "id")) {
                    if !id.is_empty() {
                        if id.len() > 1024 {
                            return Err(failure());
                        }
                        if self.seen_ids.contains(id) {
                            continue;
                        }
                        if self.seen_ids.len() >= 4096 {
                            return Err(failure());
                        }
                        self.seen_ids.insert(id.to_owned());
                    }
                }
                let value: Value = serde_json::from_str(&data).map_err(|_| failure())?;
                if value["jsonrpc"] != "2.0" || !value.is_object() {
                    return Err(failure());
                }
                // This slice never authorizes tools or user-interaction requests.
                if value.get("method").is_some() && value.get("id").is_some() {
                    return Err(failure());
                }
                return Ok(value);
            }
            let chunk = tokio::time::timeout(Duration::from_secs(30), self.stream.next())
                .await
                .map_err(|_| failure())?
                .ok_or_else(failure)?
                .map_err(|_| failure())?;
            if self.buffer.len() + self.remainder.len() + chunk.len() > FRAME_LIMIT {
                return Err(failure());
            }
            append_utf8_safe(&mut self.buffer, &mut self.remainder, &chunk);
        }
    }
    async fn result(&mut self, id: u64) -> Result<(), ProxyError> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let value = self.next().await?;
                if value.get("id").is_some_and(|v| v == id) {
                    if value.get("error").is_some()
                        || !value.get("result").is_some_and(Value::is_object)
                    {
                        return Err(failure());
                    }
                    if id == 1
                        && (value["result"]["protocolVersion"] != 1
                            || value["result"]["agentCapabilities"]["loadSession"] != true)
                    {
                        return Err(failure());
                    }
                    return Ok(());
                }
                // session/load replays old history: intentionally consume no text.
            }
        })
        .await
        .map_err(|_| failure())?
    }
}
async fn send(
    client: &reqwest::Client,
    config: &Config,
    connection: &str,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), ProxyError> {
    let response = client
        .post(&config.link)
        .bearer_auth(&config.gateway_ticket)
        .header("Acp-Connection-Id", connection)
        .timeout(Duration::from_secs(20))
        .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .send()
        .await
        .map_err(|_| failure())?;
    if !response.status().is_success() {
        return Err(failure());
    }
    Ok(())
}
fn event(value: Value) -> Bytes {
    Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        value["type"].as_str().unwrap_or("error"),
        value
    ))
}
pub(crate) async fn forward(
    app: &AppType,
    method: &http::Method,
    provider: &Provider,
    endpoint: &str,
    body: &Value,
) -> Result<ProxyResponse, ProxyError> {
    crate::proxy::managed_accounts::workbuddy_model_admission()?;
    forward_protocol(app, method, provider, endpoint, body).await
}

// Retained private ACP protocol primitive; no production caller can enter it
// without the public capability withdrawal above. Unit tests use loopback only.
async fn forward_protocol(
    app: &AppType,
    method: &http::Method,
    provider: &Provider,
    endpoint: &str,
    body: &Value,
) -> Result<ProxyResponse, ProxyError> {
    let (config, text, streaming) = input(app, method, provider, endpoint, body)?;
    let guard = session_guard(&config)?;
    let client = crate::proxy::http_client::get_authenticated().map_err(|_| failure())?;
    let response = client
        .get(&config.link)
        .bearer_auth(&config.gateway_ticket)
        .header("Accept", "text/event-stream")
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .map_err(|_| failure())?;
    if !response.status().is_success()
        || !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Err(failure());
    }
    let connection = response
        .headers()
        .get("Acp-Connection-Id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 1024)
        .ok_or_else(failure)?
        .to_owned();
    let mut events = Events {
        stream: Box::pin(response.bytes_stream()),
        buffer: String::new(),
        remainder: vec![],
        seen_ids: HashSet::new(),
    };
    tokio::try_join!(
        send(
            &client,
            &config,
            &connection,
            1,
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false}}})
        ),
        events.result(1)
    )?;
    tokio::try_join!(
        send(
            &client,
            &config,
            &connection,
            2,
            "session/load",
            json!({"sessionId":config.session_id,"cwd":"/workspace","mcpServers":[]})
        ),
        events.result(2)
    )?;
    let model = config.declared_model.clone();
    let output = async_stream::try_stream! {
        let _guard=guard;
        let id=format!("msg_workbuddy_{}",uuid::Uuid::new_v4());
        let prompt=send(&client,&config,&connection,3,"session/prompt",json!({"sessionId":config.session_id,"prompt":[{"type":"text","text":text}]}));
        tokio::pin!(prompt);
        let mut acknowledged=false;
        yield event(json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant","model":config.declared_model,"content":[]}}));
        yield event(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}));
        loop {
            let next=tokio::select! {
                ack=&mut prompt, if !acknowledged => ack.map(|_|None),
                value=events.next() => value.map(Some),
            };
            let Some(value)=next.map_err(|_|std::io::Error::other("WorkBuddy ACP stream or acknowledgement failed"))? else {
                acknowledged=true;
                continue;
            };
            if value.get("id").is_some_and(|v|v==3) {
                if value.get("error").is_some() || value["result"]["stopReason"]!="end_turn" {
                    Err(std::io::Error::other("WorkBuddy ACP prompt refused, cancelled or unsupported terminal state"))?;
                }
                if !acknowledged {prompt.as_mut().await.map_err(|_|std::io::Error::other("WorkBuddy ACP prompt acknowledgement failed"))?;}
                yield event(json!({"type":"content_block_stop","index":0}));
                yield event(json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null}}));
                yield event(json!({"type":"message_stop"}));
                break;
            }
            if value["method"]=="session/update" {
                if value["params"]["sessionId"]!=config.session_id {Err(std::io::Error::other("WorkBuddy ACP session mismatch"))?;}
                let update=&value["params"]["update"];
                if update["sessionUpdate"]=="agent_message_chunk" {
                    if update["content"]["type"]!="text" {Err(std::io::Error::other("WorkBuddy ACP non-text output unsupported"))?;}
                    let text=update["content"]["text"].as_str().ok_or_else(||std::io::Error::other("WorkBuddy ACP malformed text"))?;
                    yield event(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}));
                }
            }
        }
    };
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "content-type",
        http::HeaderValue::from_static("text/event-stream"),
    );
    let response = ProxyResponse::streamed(http::StatusCode::OK, headers, output);
    if streaming {
        Ok(response)
    } else {
        let bytes = response
            .bytes_with_limit(FRAME_LIMIT)
            .await
            .map_err(|_| failure())?;
        let text = std::str::from_utf8(&bytes).map_err(|_| failure())?;
        let result = super::transform_codex_anthropic::anthropic_sse_to_message_value(text)?;
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "content-type",
            http::HeaderValue::from_static("application/json"),
        );
        let mut result = result;
        result["model"] = json!(model);
        result.as_object_mut().ok_or_else(failure)?.remove("usage");
        Ok(ProxyResponse::buffered(
            http::StatusCode::OK,
            headers,
            Bytes::from(serde_json::to_vec(&result).map_err(|_| failure())?),
        ))
    }
}
#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(
    any(feature = "gui", test),
    any(test, debug_assertions, feature = "test-hooks")
))]
#[path = "workbuddy_acp_pause_fixture.rs"]
pub(crate) mod pause_fixture;
