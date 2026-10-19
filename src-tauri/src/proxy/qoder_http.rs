//! Per-request Qoder HTTP consumer. Mature protocol conversion runs in the
//! application-owned Node worker; account state remains in the original vault.
mod request;
mod transport;
mod wire;
use super::{error::ProxyError, hyper_client::ProxyResponse, qoder_host::HostConfig};
use crate::{database::Database, services::provider::qoder_http_auth::RequestSnapshot};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroize;

pub(crate) fn enabled(provider: &crate::provider::Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        == Some("qoder_sdk")
}

pub(crate) fn chat_request(body: &Value, endpoint: &str) -> Result<Value, ProxyError> {
    request::chat_request(body, endpoint)
}

pub(crate) struct Dispatch {
    pub host: HostConfig,
    pub database: Arc<Database>,
    pub snapshot: Arc<RequestSnapshot>,
    pub first_byte_timeout: u64,
    pub idle_timeout: u64,
    pub non_streaming_timeout: u64,
}

struct PrivateRequest(Value);
impl Drop for PrivateRequest {
    fn drop(&mut self) {
        fn clear(value: &mut Value) {
            match value {
                Value::String(text) => text.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(clear),
                Value::Object(values) => values.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.0);
    }
}

pub(crate) async fn forward(body: Value, dispatch: Dispatch) -> Result<ProxyResponse, ProxyError> {
    dispatch
        .database
        .check_qoder_http_snapshot(&dispatch.snapshot, chrono::Utc::now().timestamp_millis())
        .map_err(|_| ProxyError::Internal("qoder.http_auth.authorization_changed".into()))?;
    let streaming = body["stream"] == true;
    // Backend-private pipe only. No PAT/refresh, executable path, caller token,
    // or alternate endpoint is accepted from the incoming HTTP request.
    let private_request = PrivateRequest(json!({
        "binding": {
            "accountId": dispatch.snapshot.stamp.source.provider_id,
            "sessionId": uuid::Uuid::new_v4().to_string(),
            "turnId": uuid::Uuid::new_v4().to_string()
        },
        "snapshot": {
            "access": dispatch.snapshot.access_token(),
            "uid": dispatch.snapshot.uid(),
            "machineId": dispatch.snapshot.machine_id(),
            "expiry": dispatch.snapshot.expires_at_ms(),
            "region": dispatch.snapshot.region()
        },
        "chat": body
    }));
    transport::forward(
        &dispatch.host,
        &private_request.0,
        streaming,
        Duration::from_secs(dispatch.first_byte_timeout),
        Duration::from_secs(dispatch.idle_timeout),
        Duration::from_secs(dispatch.non_streaming_timeout),
    )
    .await
}
