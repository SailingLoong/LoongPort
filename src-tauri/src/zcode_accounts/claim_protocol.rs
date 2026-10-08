//! Fixed claim transport adapted from pjpv/zcode-switch f34225686dfef05d84c256a56f868719248f15ff.
//! Copyright (c) 2026 zcode-switch contributors; MIT, see licenses/zcode-switch-MIT.txt.
use super::official::{
    OfficialError, OfficialResponse, RequestCheck, SecretValue, StartJwt, TransportFuture,
    MAX_RESPONSE_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Preview,
    Balance,
    Config,
    Claim,
}
pub(crate) struct Request {
    pub endpoint: Endpoint,
    pub version: String,
    pub jwt: StartJwt,
    pub plan: Option<String>,
    pub captcha: Option<SecretValue>,
    pub region: Option<String>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimRequest")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
pub(crate) trait Transport: Send + Sync {
    fn send(&self, request: Request) -> TransportFuture<'_>;
}
pub(crate) struct Client<T>(pub T);
impl<T: Transport> Client<T> {
    pub async fn request(
        &self,
        request: Request,
        check: &RequestCheck<'_>,
    ) -> Result<Value, OfficialError> {
        check()?;
        let jwt = request.jwt.clone();
        let captcha = request.captcha.clone();
        let response = self.0.send(request).await?;
        check()?;
        if matches!(response.status, 401 | 403) {
            return Err(OfficialError::Unauthorized);
        }
        if !(200..300).contains(&response.status) {
            return Err(OfficialError::Http(response.status));
        }
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(OfficialError::ResponseTooLarge);
        }
        let mut value: Value =
            serde_json::from_slice(&response.body).map_err(|_| OfficialError::InvalidResponse)?;
        if !value.is_object() {
            return Err(OfficialError::InvalidResponse);
        }
        fn scrub(v: &mut Value, jwt: &str, captcha: Option<&str>) {
            match v {
                Value::String(s) if s.contains(jwt) || captcha.is_some_and(|c| s.contains(c)) => {
                    use zeroize::Zeroize;
                    s.zeroize();
                    *v = Value::Null;
                }
                Value::Array(a) => a.iter_mut().for_each(|v| scrub(v, jwt, captcha)),
                Value::Object(o) => o.values_mut().for_each(|v| scrub(v, jwt, captcha)),
                _ => {}
            }
        }
        scrub(
            &mut value,
            jwt.expose(),
            captcha.as_ref().map(SecretValue::expose),
        );
        Ok(value)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaptchaConfig {
    pub enabled: bool,
    pub region: String,
    pub prefix: String,
    pub scene_id: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    struct Fake {
        calls: AtomicUsize,
        response: Mutex<Option<Result<OfficialResponse, OfficialError>>>,
    }
    impl Transport for Fake {
        fn send(&self, _request: Request) -> TransportFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let r = self.response.lock().unwrap().take().unwrap();
            Box::pin(async move { r })
        }
    }
    fn req() -> Request {
        Request {
            endpoint: Endpoint::Preview,
            version: "3.11.2".into(),
            jwt: StartJwt::new("secret-canary").unwrap(),
            plan: None,
            captcha: None,
            region: None,
        }
    }
    #[tokio::test]
    async fn cancellation_admits_no_network_request() {
        let fake = Fake {
            calls: AtomicUsize::new(0),
            response: Mutex::new(None),
        };
        let client = Client(fake);
        assert_eq!(
            client
                .request(req(), &|| Err(OfficialError::Cancelled))
                .await
                .unwrap_err(),
            OfficialError::Cancelled
        );
        assert_eq!(client.0.calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn timeout_is_single_attempt_and_secret_response_is_redacted() {
        let fake = Fake {
            calls: AtomicUsize::new(0),
            response: Mutex::new(Some(Err(OfficialError::Timeout))),
        };
        let client = Client(fake);
        assert_eq!(
            client.request(req(), &|| Ok(())).await.unwrap_err(),
            OfficialError::Timeout
        );
        assert_eq!(client.0.calls.load(Ordering::SeqCst), 1);
        let fake = Fake {
            calls: AtomicUsize::new(0),
            response: Mutex::new(Some(Ok(OfficialResponse {
                status: 200,
                body: zeroize::Zeroizing::new(
                    br#"{"code":0,"data":{"name":"secret-canary"}}"#.to_vec(),
                ),
            }))),
        };
        let value = Client(fake).request(req(), &|| Ok(())).await.unwrap();
        assert!(!value.to_string().contains("secret-canary"));
        assert!(!format!("{:?}", req()).contains("secret-canary"));
    }
}

#[cfg(feature = "gui")]
pub(crate) struct Http {
    client: reqwest::Client,
}
#[cfg(feature = "gui")]
impl Http {
    pub fn new() -> Result<Self, OfficialError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(25))
                .build()
                .map_err(|_| OfficialError::Transport)?,
        })
    }
}
#[cfg(feature = "gui")]
impl Transport for Http {
    fn send(&self, request: Request) -> TransportFuture<'_> {
        Box::pin(async move {
            let path = match request.endpoint {
                Endpoint::Preview => "zcode-plan/billing/preview",
                Endpoint::Balance => "zcode-plan/billing/balance",
                Endpoint::Config => "client/configs",
                Endpoint::Claim => "zcode-plan/billing/claim",
            };
            if request.version.is_empty()
                || request.version.len() > 64
                || !request
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
            {
                return Err(OfficialError::InvalidInput);
            }
            let mut url = url::Url::parse(&format!("https://zcode.z.ai/api/v1/{path}"))
                .map_err(|_| OfficialError::InvalidInput)?;
            let os = match std::env::consts::OS {
                "macos" => "darwin",
                other => other,
            };
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                other => other,
            };
            let platform = format!("{os}-{arch}");
            if matches!(request.endpoint, Endpoint::Preview | Endpoint::Balance) {
                url.query_pairs_mut()
                    .append_pair("app_version", &request.version);
            }
            if request.endpoint == Endpoint::Preview {
                url.query_pairs_mut().append_pair("platform", &platform);
            }
            let mut auth =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", request.jwt.expose()))
                    .map_err(|_| OfficialError::InvalidInput)?;
            auth.set_sensitive(true);
            let mut builder = if request.endpoint == Endpoint::Claim {
                self.client.post(url)
            } else {
                self.client.get(url)
            };
            builder = builder
                .header(reqwest::header::AUTHORIZATION, auth)
                .header("X-ZCode-App-Version", &request.version)
                .header("X-Platform", platform)
                .header("X-Title", "LoongPort")
                .header(
                    "User-Agent",
                    concat!("LoongPort/", env!("CARGO_PKG_VERSION")),
                );
            if request.endpoint == Endpoint::Claim {
                let captcha = request
                    .captcha
                    .as_ref()
                    .ok_or(OfficialError::InvalidInput)?;
                let mut header = reqwest::header::HeaderValue::from_str(captcha.expose())
                    .map_err(|_| OfficialError::InvalidInput)?;
                header.set_sensitive(true);
                builder = builder.header("X-Aliyun-Captcha-Verify-Param", header);
                if let Some(region) = request.region {
                    builder = builder.header("X-Aliyun-Captcha-Verify-Region", region);
                }
                let plan = request.plan.ok_or(OfficialError::InvalidInput)?;
                builder = builder.json(&serde_json::json!({"plan_id":plan}));
            }
            let error = |e: reqwest::Error| {
                if e.is_timeout() {
                    OfficialError::Timeout
                } else {
                    OfficialError::Transport
                }
            };
            let mut response = builder.send().await.map_err(error)?;
            if response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
            {
                return Err(OfficialError::ResponseTooLarge);
            }
            let status = response.status().as_u16();
            let mut body = zeroize::Zeroizing::new(Vec::new());
            while let Some(chunk) = response.chunk().await.map_err(error)? {
                if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
                    return Err(OfficialError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(OfficialResponse { status, body })
        })
    }
}
pub(crate) fn captcha_config(value: &Value) -> Result<CaptchaConfig, OfficialError> {
    if matches!(value.get("code").and_then(Value::as_i64), Some(401 | 403)) {
        return Err(OfficialError::Unauthorized);
    }
    if value.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(OfficialError::BusinessRejected);
    }
    let c = value
        .pointer("/data/configs/captcha")
        .ok_or(OfficialError::InvalidResponse)?;
    let field = |key| {
        c.get(key)
            .and_then(Value::as_str)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
            .map(String::from)
            .ok_or(OfficialError::InvalidResponse)
    };
    Ok(CaptchaConfig {
        enabled: c.get("enabled").and_then(Value::as_bool) == Some(true),
        region: field("region")?,
        prefix: field("prefix")?,
        scene_id: field("sceneId")?,
    })
}

pub(crate) fn read_data(value: Value) -> Result<Value, OfficialError> {
    if matches!(value.get("code").and_then(Value::as_i64), Some(401 | 403)) {
        return Err(OfficialError::Unauthorized);
    }
    if value
        .get("code")
        .and_then(Value::as_i64)
        .is_some_and(|c| c != 0 && c != 200)
    {
        return Err(OfficialError::BusinessRejected);
    }
    value
        .get("data")
        .cloned()
        .ok_or(OfficialError::InvalidResponse)
}
#[cfg(test)]
mod business_error_tests {
    use super::*;
    #[test]
    fn business_auth_rejection_is_login_expiry_on_reads_and_captcha() {
        let v = serde_json::json!({"code":401,"data":{}});
        assert_eq!(
            read_data(v.clone()).unwrap_err(),
            OfficialError::Unauthorized
        );
        assert_eq!(captcha_config(&v).unwrap_err(), OfficialError::Unauthorized);
    }
}
