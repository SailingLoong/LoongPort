//! Wire reference: MiniMax-AI/minimax-code 9a935605956c4500e9cb415f29258e2887e97817.
//! Original MIT notice: licenses/minimax-code-MIT.txt.
use super::catalog::{Failure, Region};
use serde_json::Value;
use std::{future::Future, pin::Pin};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    DeviceCode,
    Token,
    Profile,
    Refresh,
    Status,
    Claim,
}
pub(crate) struct Request {
    pub endpoint: Endpoint,
    pub region: Region,
    pub access_token: Option<String>,
    pub real_user_id: Option<String>,
    pub body: Option<Value>,
}
pub(crate) struct Response {
    pub status: u16,
    pub body: Value,
}
pub(crate) trait Transport: Send + Sync {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>>;
}
pub(crate) struct OfficialHttp;

use md5::{Digest, Md5};
use std::time::Duration;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
fn md5(value: &str) -> String {
    hex::encode(Md5::digest(value.as_bytes()))
}
// JavaScript encodeURIComponent, which differs from form-query encoding.
fn component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char)
        } else {
            out.push('%');
            out.push(b"0123456789ABCDEF"[(b >> 4) as usize] as char);
            out.push(b"0123456789ABCDEF"[(b & 15) as usize] as char)
        }
    }
    out
}
pub(super) fn new_client(timeout: Duration) -> Result<reqwest::Client, Failure> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .build()
        .map_err(|_| Failure::NetworkUnavailable)
}
pub(super) fn build_request(
    client: &reqwest::Client,
    r: Request,
    now: i64,
    timezone: i32,
    os: &str,
    version: &str,
) -> Result<reqwest::Request, Failure> {
    let account_origin = match r.region {
        Region::Cn => "https://account.minimax.cn",
        Region::Global => "https://account.minimax.io",
    };
    let agent_origin = match r.region {
        Region::Cn => "https://agent.minimaxi.com",
        Region::Global => "https://agent.minimax.io",
    };
    let path = match r.endpoint {
        Endpoint::DeviceCode => "/oauth2/device/code",
        Endpoint::Token | Endpoint::Refresh => "/oauth2/token",
        Endpoint::Profile => "/v1/api/user/info",
        Endpoint::Status => "/minimax-cloud/api/v1/signin/status",
        Endpoint::Claim => "/minimax-cloud/api/v1/signin/claim",
    };
    if matches!(
        r.endpoint,
        Endpoint::DeviceCode | Endpoint::Token | Endpoint::Refresh
    ) {
        let values = r
            .body
            .as_ref()
            .and_then(Value::as_object)
            .ok_or(Failure::InvalidResponse)?
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.as_str(), v))
                    .ok_or(Failure::InvalidResponse)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return client
            .post(format!("{account_origin}{path}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&values)
            .build()
            .map_err(|_| Failure::InvalidResponse);
    }
    let token = r
        .access_token
        .as_deref()
        .filter(|s| !s.trim().is_empty() && s.len() <= 16384)
        .ok_or(Failure::NeedsLogin)?;
    let uid = if r.endpoint == Endpoint::Profile {
        "0"
    } else {
        r.real_user_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .ok_or(Failure::NeedsLogin)?
    };
    let language = if r.region == Region::Cn { "zh" } else { "en" };
    let time = now.to_string();
    let timezone = timezone.to_string();
    let mut pairs = vec![
        (
            "device_platform",
            if r.endpoint == Endpoint::Profile {
                "mcode"
            } else {
                "web"
            },
        ),
        ("biz_id", "3"),
        ("app_id", "3001"),
        ("version_code", "22201"),
    ];
    if r.endpoint != Endpoint::Profile {
        pairs.extend([("is_desktop", "1"), ("desktop_version", version.trim())]);
    }
    pairs.extend([
        ("unix", time.as_str()),
        ("timezone_offset", timezone.as_str()),
        ("sys_language", language),
        ("lang", language),
        ("device_id", "0"),
        ("os_name", os),
        ("browser_name", "mcode"),
        ("user_id", uid),
        ("client", "mcode"),
    ]);
    let mut url = reqwest::Url::parse(&format!("{agent_origin}{path}"))
        .map_err(|_| Failure::UnsupportedContext)?;
    url.query_pairs_mut().extend_pairs(pairs);
    let body = if r.endpoint == Endpoint::Claim {
        if r.body != Some(serde_json::json!({})) {
            return Err(Failure::InvalidResponse);
        }
        Some("{}")
    } else {
        None
    };
    let path_query = format!("{}?{}", url.path(), url.query().unwrap_or(""));
    let second = (now / 1000).to_string();
    let yy = md5(&format!(
        "{}_{}{}ooui",
        component(&path_query),
        body.unwrap_or("{}"),
        md5(&time)
    ));
    let signature = md5(&format!(
        "{second}I*7Cf%WZ#S&%1RlZJ&C2{}",
        body.unwrap_or("")
    ));
    let mut request = client
        .request(
            if r.endpoint == Endpoint::Claim {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            url,
        )
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, "MiniMaxCode")
        .bearer_auth(token)
        .header("yy", yy)
        .header("x-timestamp", second)
        .header("x-signature", signature);
    if let Some(body) = body {
        request = request.body(body)
    }
    request.build().map_err(|_| Failure::InvalidResponse)
}
pub(super) async fn send_once(
    client: &reqwest::Client,
    request: reqwest::Request,
) -> Result<Response, Failure> {
    let mut response = client
        .execute(request)
        .await
        .map_err(|_| Failure::NetworkUnavailable)?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Failure::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::NetworkUnavailable)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(Failure::InvalidResponse);
        }
        body.extend_from_slice(&chunk)
    }
    let body = match serde_json::from_slice(&body) {
        Ok(body) => body,
        // HTTP authentication status remains meaningful with an HTML/text error
        // page. It may refresh credentials, but never authorizes a POST replay.
        Err(_) if !(200..300).contains(&status) => Value::Null,
        Err(_) => return Err(Failure::InvalidResponse),
    };
    Ok(Response { status, body })
}
impl Transport for OfficialHttp {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            let client = new_client(Duration::from_secs(30))?;
            let os = if cfg!(target_os = "windows") {
                "win32"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else {
                std::env::consts::OS
            };
            let request = build_request(
                &client,
                request,
                super::catalog::now_ms(),
                chrono::Local::now().offset().local_minus_utc(),
                os,
                env!("CARGO_PKG_VERSION"),
            )?;
            send_once(&client, request).await
        })
    }
}
