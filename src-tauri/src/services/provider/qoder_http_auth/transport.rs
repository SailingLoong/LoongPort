//! Fixed region-bound PAT HTTP authentication. Protocol source: Magpie cdf45137,
//! openapi 167-180, PAT exchange/userinfo 308-365, refreshJob 383-395.
//! CN endpoint facts: minglu6/dsh-provider-qoder src/pat.ts and src/cosy.ts (MIT).
//! No caller host, VPC, native browser flow, fallback, retry, or logging.
use super::*;
use crate::services::provider::qoder_auth_http;
use reqwest::{header, Method, Url};
use serde_json::Value;
use std::time::Duration;
const MAX_BYTES: usize = 1024 * 1024;
const DAY_MS: i64 = 86_400_000;

pub(crate) struct FixedRegionTransport {
    client: reqwest::Client,
    source: SourceKind,
    #[cfg(test)]
    test_origin: Option<Url>,
}
#[derive(Clone, Copy)]
enum Endpoint {
    Exchange,
    Userinfo,
    Refresh,
}
impl Endpoint {
    fn path(self) -> &'static str {
        match self {
            Self::Exchange => qoder_auth_http::PAT_EXCHANGE_PATH,
            Self::Userinfo => "/api/v1/userinfo",
            Self::Refresh => "/api/v1/jobToken/refresh",
        }
    }
    fn method(self) -> Method {
        if matches!(self, Self::Userinfo) {
            Method::GET
        } else {
            Method::POST
        }
    }
}
/// Unlike ordinary Value, response/request token strings are erased on drop.
struct SensitiveJson(Value);
impl Drop for SensitiveJson {
    fn drop(&mut self) {
        fn clear(v: &mut Value) {
            match v {
                Value::String(s) => s.zeroize(),
                Value::Array(a) => a.iter_mut().for_each(clear),
                Value::Object(o) => o.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.0);
    }
}
impl FixedRegionTransport {
    pub(crate) fn for_source(source: SourceKind) -> Result<Self, AppError> {
        let client = qoder_auth_http::client_builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| denied("transport_unavailable"))?;
        Ok(Self {
            client,
            source,
            #[cfg(test)]
            test_origin: None,
        })
    }
    fn origin(&self) -> Result<Url, ()> {
        #[cfg(test)]
        if let Some(origin) = &self.test_origin {
            return Ok(origin.clone());
        }
        Url::parse(self.source.origin()).map_err(|_| ())
    }
    /// The only alternate-origin factory is absent from production builds.
    #[cfg(test)]
    fn synthetic_server(origin: Url) -> Result<Self, ()> {
        if origin.scheme() != "http"
            || origin.host_str() != Some("127.0.0.1")
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(());
        }
        let client = qoder_auth_http::client_builder()
            .timeout(Duration::from_secs(20))
            .no_proxy()
            .build()
            .map_err(|_| ())?;
        Ok(Self {
            client,
            source: SourceKind::InternationalQoderPat,
            test_origin: Some(origin),
        })
    }
    async fn request(
        &self,
        endpoint: Endpoint,
        bearer: Option<&str>,
        body: Option<SensitiveJson>,
    ) -> Result<(SensitiveJson, i64), ()> {
        let origin = self.origin()?;
        let url = origin.join(endpoint.path()).map_err(|_| ())?;
        let mut request = self
            .client
            .request(endpoint.method(), url.clone())
            .header(header::ACCEPT, "application/json");
        if self.source == SourceKind::ChinaQoderPat {
            request = request
                .header("Cosy-Version", "1.0.1")
                .header("Cosy-ClientType", "5")
                .header(header::USER_AGENT, "LoongPort");
        }
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        if let Some(body) = &body {
            request = request.json(&body.0);
        }
        drop(body);
        let mut response = request.send().await.map_err(|_| ())?;
        if response.url() != &url
            || response.url().origin() != origin.origin()
            || response.status().is_redirection()
        {
            return Err(());
        }
        let accepted = if matches!(endpoint, Endpoint::Userinfo) {
            response.status().is_success()
        } else {
            response.status() == reqwest::StatusCode::OK
        };
        if !accepted
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(());
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        let json = SensitiveJson(serde_json::from_slice(&bytes).map_err(|_| ())?);
        if !json.0.is_object() {
            return Err(());
        }
        Ok((json, chrono::Utc::now().timestamp_millis()))
    }
    async fn establish_inner(&self, pat: &str, machine_id: &str) -> Result<Established, ()> {
        if pat.trim().is_empty() || uuid::Uuid::parse_str(machine_id).is_err() {
            return Err(());
        }
        let (exchange, received_at) = self
            .request(
                Endpoint::Exchange,
                None,
                Some(SensitiveJson(qoder_auth_http::pat_exchange_body(pat))),
            )
            .await?;
        let tokens = parse_exchange_for_source(&self.source, &exchange.0, received_at)?;
        let (userinfo, _) = self
            .request(Endpoint::Userinfo, Some(&tokens.access_token), None)
            .await?;
        let uid = parse_uid(&userinfo.0)?;
        Ok(Established { uid, tokens })
    }
    async fn refresh_inner(&self, refresh: &str, identity: &Identity) -> Result<TokenPair, ()> {
        if refresh.trim().is_empty()
            || identity.uid.trim().is_empty()
            || uuid::Uuid::parse_str(&identity.machine_id).is_err()
        {
            return Err(());
        }
        let (response, received_at) = self
            .request(
                Endpoint::Refresh,
                None,
                Some(SensitiveJson(serde_json::json!({"refresh_token":refresh}))),
            )
            .await?;
        parse_refresh(&response.0, received_at)
    }
}
impl AuthTransport for FixedRegionTransport {
    fn source_kind(&self) -> SourceKind {
        self.source.clone()
    }
    fn exchange_and_identify<'a>(
        &'a self,
        pat: &'a str,
        machine_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>> {
        Box::pin(self.establish_inner(pat, machine_id))
    }
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        identity: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>> {
        Box::pin(self.refresh_inner(refresh_token, identity))
    }
}
fn first_non_null<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| v.get(*key).filter(|value| !value.is_null()))
}
fn token(value: Option<&Value>, optional: bool) -> Result<String, ()> {
    let Some(value) = value else {
        return if optional { Ok(String::new()) } else { Err(()) };
    };
    let value = value.as_str().ok_or(())?.trim();
    if (!optional && value.is_empty()) || value.len() > 16384 || value.chars().any(char::is_control)
    {
        return Err(());
    }
    Ok(value.to_owned())
}
fn relative_expiry(body: &Value, now: i64) -> Result<i64, ()> {
    let duration = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(DAY_MS);
    now.checked_add(duration)
        .filter(|value| *value > now && now >= 0)
        .ok_or(())
}
fn exchange_expiry(body: &Value, now: i64) -> Result<i64, ()> {
    for key in ["expire_time", "expires_at"] {
        if let Some(value) = body.get(key).and_then(Value::as_str) {
            // Explicit-offset protocol dates only; never infer local time zones.
            if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value.trim()) {
                return Ok(date.timestamp_millis());
            }
            if let Ok(date) = chrono::DateTime::parse_from_rfc2822(value.trim()) {
                return Ok(date.timestamp_millis());
            }
        }
    }
    relative_expiry(body, now)
}
fn parse_exchange_for_source(source: &SourceKind, body: &Value, now: i64) -> Result<TokenPair, ()> {
    if *source == SourceKind::InternationalQoderPat {
        return parse_exchange(body, now);
    }
    // Deliberately narrower than the official CLI's mixed-family aliases.
    // This owner handles only independently supplied CN PAT job credentials,
    // never imported device/login tokens or their refresh ownership.
    let tokens = TokenPair {
        access_token: token(body.get("token"), false)?,
        refresh_token: token(body.get("refresh_token"), true)?,
        expires_at_ms: exchange_expiry(body, now)?,
    };
    validate_tokens(&tokens, now).map_err(|_| ())?;
    Ok(tokens)
}
fn parse_exchange(body: &Value, now: i64) -> Result<TokenPair, ()> {
    let tokens = TokenPair {
        access_token: token(
            first_non_null(body, &["token", "device_token", "access_token"]),
            false,
        )?,
        refresh_token: token(
            first_non_null(body, &["refresh_token", "refreshToken"]),
            true,
        )?,
        expires_at_ms: exchange_expiry(body, now)?,
    };
    validate_tokens(&tokens, now).map_err(|_| ())?;
    Ok(tokens)
}
fn parse_refresh(body: &Value, now: i64) -> Result<TokenPair, ()> {
    let tokens = TokenPair {
        access_token: token(body.get("token"), false)?,
        refresh_token: token(body.get("refresh_token"), false)?,
        expires_at_ms: relative_expiry(body, now)?,
    };
    validate_tokens(&tokens, now).map_err(|_| ())?;
    Ok(tokens)
}
fn parse_uid(body: &Value) -> Result<String, ()> {
    let value = first_non_null(body, &["id", "user_id", "uid"]).ok_or(())?;
    let uid = match value {
        Value::String(value) => value.trim().to_owned(),
        Value::Number(value) => value.to_string(),
        _ => return Err(()),
    };
    if uid.is_empty() || uid.len() > 1024 || uid.chars().any(char::is_control) {
        return Err(());
    }
    Ok(uid)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
