//! Fixed region-bound PAT/device HTTP authentication. Protocol source: Magpie cdf45137,
//! openapi 167-180, PAT exchange/userinfo 308-365, refreshJob 383-395.
//! CN endpoint facts: minglu6/dsh-provider-qoder src/pat.ts and src/cosy.ts (MIT).
//! No caller host, VPC, fallback, automatic retry, or logging. Device grants are
//! owned by an independent session; poll success requires authenticated UID matching.
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
    DevicePoll,
    DeviceRefresh,
}
impl Endpoint {
    fn path(self) -> &'static str {
        match self {
            Self::Exchange => qoder_auth_http::PAT_EXCHANGE_PATH,
            Self::Userinfo => "/api/v1/userinfo",
            Self::Refresh => "/api/v1/jobToken/refresh",
            Self::DevicePoll => "/api/v1/deviceToken/poll",
            Self::DeviceRefresh => "/api/v1/deviceToken/refresh",
        }
    }
    fn method(self) -> Method {
        if matches!(self, Self::Userinfo | Self::DevicePoll) {
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
    fn device_poll_request(&self, nonce: &str, verifier: &str) -> Result<reqwest::Request, ()> {
        if self.source != SourceKind::ChinaQoderDevice
            || uuid::Uuid::parse_str(nonce).is_err()
            || !(43..=128).contains(&verifier.len())
            || !verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(());
        }
        self.build_request(Endpoint::DevicePoll, None, None)?
            .query(&[
                ("nonce", nonce),
                ("verifier", verifier),
                ("challenge_method", "S256"),
            ])
            .build()
            .map_err(|_| ())
    }
    fn refresh_request(&self, refresh: &str, identity: &Identity) -> Result<reqwest::Request, ()> {
        if refresh.trim().is_empty()
            || identity.uid.trim().is_empty()
            || uuid::Uuid::parse_str(&identity.machine_id).is_err()
        {
            return Err(());
        }
        let native = self.source == SourceKind::ChinaQoderDevice;
        let body = if native {
            // Same independently owned machine ID as login. Never invent machine_token.
            serde_json::json!({"refresh_token":refresh,"machine_id":identity.machine_id})
        } else {
            serde_json::json!({"refresh_token":refresh})
        };
        self.build_request(
            if native {
                Endpoint::DeviceRefresh
            } else {
                Endpoint::Refresh
            },
            None,
            Some(SensitiveJson(body)),
        )?
        .build()
        .map_err(|_| ())
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
    fn build_request(
        &self,
        endpoint: Endpoint,
        bearer: Option<&str>,
        body: Option<SensitiveJson>,
    ) -> Result<reqwest::RequestBuilder, ()> {
        let url = self.origin()?.join(endpoint.path()).map_err(|_| ())?;
        let mut request = self
            .client
            .request(endpoint.method(), url)
            .header(header::ACCEPT, "application/json");
        if matches!(
            self.source,
            SourceKind::ChinaQoderPat | SourceKind::ChinaQoderDevice
        ) {
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
        Ok(request)
    }
    async fn send_request(
        &self,
        endpoint: Endpoint,
        request: reqwest::Request,
    ) -> Result<Option<(SensitiveJson, i64)>, ()> {
        let url = request.url().clone();
        let origin = self.origin()?;
        if url.origin() != origin.origin() || url.path() != endpoint.path() {
            return Err(());
        }
        let mut response = self.client.execute(request).await.map_err(|_| ())?;
        if response.url() != &url
            || response.url().origin() != origin.origin()
            || response.status().is_redirection()
        {
            return Err(());
        }
        if response_status(endpoint, response.status())? {
            return Ok(None);
        }
        if response
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
        Ok(Some((json, chrono::Utc::now().timestamp_millis())))
    }
    async fn request(
        &self,
        endpoint: Endpoint,
        bearer: Option<&str>,
        body: Option<SensitiveJson>,
    ) -> Result<(SensitiveJson, i64), ()> {
        let request = self
            .build_request(endpoint, bearer, body)?
            .build()
            .map_err(|_| ())?;
        self.send_request(endpoint, request).await?.ok_or(())
    }
    pub(crate) async fn poll_device_login(
        &self,
        nonce: &str,
        verifier: &str,
    ) -> Result<Option<Established>, ()> {
        let request = self.device_poll_request(nonce, verifier)?;
        let Some((poll, received_at)) = self.send_request(Endpoint::DevicePoll, request).await?
        else {
            return Ok(None);
        };
        let (expected_uid, tokens) = parse_device_poll(&poll.0, received_at)?;
        // Official mandatory auth-status resolves via authenticated /userinfo.
        // A poll token/UID alone never establishes a publishable account identity.
        let (userinfo, _) = self
            .request(Endpoint::Userinfo, Some(&tokens.access_token), None)
            .await?;
        verify_device_identity(&expected_uid, tokens, &userinfo.0).map(Some)
    }
    async fn establish_inner(&self, pat: &str, machine_id: &str) -> Result<Established, ()> {
        if self.source == SourceKind::ChinaQoderDevice
            || pat.trim().is_empty()
            || uuid::Uuid::parse_str(machine_id).is_err()
        {
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
        let native = self.source == SourceKind::ChinaQoderDevice;
        let request = self.refresh_request(refresh, identity)?;
        let (response, received_at) = self
            .send_request(
                if native {
                    Endpoint::DeviceRefresh
                } else {
                    Endpoint::Refresh
                },
                request,
            )
            .await?
            .ok_or(())?;
        if native {
            parse_device_refresh(&response.0, received_at)
        } else {
            parse_refresh(&response.0, received_at)
        }
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

// The official CLI treats poll expires_in as seconds, unlike PAT exchange.
// Refresh uses explicit expires_at only, never PAT's guessed default lifetime.
fn device_expiry(body: &Value, now: i64, allow_relative: bool) -> Result<i64, ()> {
    let expires_at_ms = match body.get("expires_at") {
        Some(Value::String(value)) => chrono::DateTime::parse_from_rfc3339(value)
            .map_err(|_| ())?
            .timestamp_millis(),
        Some(Value::Number(value)) if !allow_relative => {
            let value = value.as_i64().filter(|v| *v > 0).ok_or(())?;
            // Verified official refresh EU(): > 1e12 is milliseconds, otherwise seconds.
            let millis = if value > 1_000_000_000_000 {
                value
            } else {
                value.checked_mul(1000).ok_or(())?
            };
            chrono::DateTime::from_timestamp_millis(millis).ok_or(())?;
            millis
        }
        None if allow_relative => {
            let seconds = body
                .get("expires_in")
                .and_then(Value::as_i64)
                .filter(|v| *v > 0)
                .ok_or(())?;
            now.checked_add(seconds.checked_mul(1000).ok_or(())?)
                .ok_or(())?
        }
        _ => return Err(()),
    };
    if now < 0 || expires_at_ms <= now {
        return Err(());
    }
    Ok(expires_at_ms)
}
fn parse_device_poll(body: &Value, now: i64) -> Result<(String, TokenPair), ()> {
    let uid = parse_uid(&serde_json::json!({"user_id": body.get("user_id")}))?;
    let tokens = TokenPair {
        access_token: token(body.get("token"), false)?,
        refresh_token: token(body.get("refresh_token"), true)?,
        expires_at_ms: device_expiry(body, now, true)?,
    };
    validate_tokens(&tokens, now).map_err(|_| ())?;
    Ok((uid, tokens))
}
fn parse_device_refresh(body: &Value, now: i64) -> Result<TokenPair, ()> {
    let tokens = TokenPair {
        access_token: token(first_non_null(body, &["device_token", "token"]), false)?,
        refresh_token: token(body.get("refresh_token"), false)?,
        expires_at_ms: device_expiry(body, now, false)?,
    };
    validate_tokens(&tokens, now).map_err(|_| ())?;
    Ok(tokens)
}
fn verify_device_identity(
    expected: &str,
    tokens: TokenPair,
    userinfo: &Value,
) -> Result<Established, ()> {
    let uid = parse_uid(userinfo)?;
    if uid != expected {
        return Err(());
    }
    Ok(Established { uid, tokens })
}

/// True means a device grant is still pending, not an authenticated response.
fn response_status(endpoint: Endpoint, status: reqwest::StatusCode) -> Result<bool, ()> {
    if matches!(endpoint, Endpoint::DevicePoll) && status == reqwest::StatusCode::NOT_FOUND {
        return Ok(true);
    }
    if status == reqwest::StatusCode::OK
        || (matches!(endpoint, Endpoint::Userinfo) && status.is_success())
    {
        Ok(false)
    } else {
        Err(())
    }
}
