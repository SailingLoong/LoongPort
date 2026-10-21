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
    UserExtra,
    Membership,
    PlanQuota,
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
    if r.endpoint == Endpoint::PlanQuota {
        // Code OAuth is not the API-key adapter: use its verified official
        // open-platform origins and personal group, without Matrix signing.
        let origin = match r.region {
            Region::Cn => "https://www.minimaxi.com",
            Region::Global => "https://platform.minimax.io",
        };
        let token = r
            .access_token
            .as_deref()
            .filter(|v| !v.trim().is_empty() && v.len() <= 16384)
            .ok_or(Failure::NeedsLogin)?;
        let group = r
            .body
            .as_ref()
            .and_then(|v| v.get("op_group_id"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
            .ok_or(Failure::UnsupportedContext)?;
        let mut group = reqwest::header::HeaderValue::from_str(group)
            .map_err(|_| Failure::UnsupportedContext)?;
        group.set_sensitive(true);
        return client
            .get(format!("{origin}/v1/api/openplatform/coding_plan/remains"))
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(token)
            .header("X-Group-Id", group)
            .build()
            .map_err(|_| Failure::InvalidResponse);
    }
    let account_origin = match r.region {
        Region::Cn => "https://account.minimax.cn",
        Region::Global => "https://account.minimax.io",
    };
    let agent_origin = match r.region {
        Region::Cn => "https://agent.minimaxi.com",
        Region::Global => "https://agent.minimax.io",
    };
    let path = match r.endpoint {
        Endpoint::PlanQuota => return Err(Failure::UnsupportedContext),
        Endpoint::DeviceCode => "/oauth2/device/code",
        Endpoint::Token | Endpoint::Refresh => "/oauth2/token",
        Endpoint::Profile => "/v1/api/user/info",
        Endpoint::Status => "/minimax-cloud/api/v1/signin/status",
        Endpoint::Claim => "/minimax-cloud/api/v1/signin/claim",
        Endpoint::UserExtra => "/matrix/api/v1/user/get_user_extra_info",
        Endpoint::Membership => "/matrix/api/v1/commerce/get_membership_info",
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
            if matches!(
                r.endpoint,
                Endpoint::Profile | Endpoint::UserExtra | Endpoint::Membership
            ) {
                "mcode"
            } else {
                "web"
            },
        ),
        ("biz_id", "3"),
        ("app_id", "3001"),
        ("version_code", "22201"),
    ];
    if !matches!(
        r.endpoint,
        Endpoint::Profile | Endpoint::UserExtra | Endpoint::Membership
    ) {
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
    let matrix_body = if matches!(r.endpoint, Endpoint::UserExtra | Endpoint::Membership) {
        let value = r
            .body
            .as_ref()
            .filter(|v| v.is_object())
            .ok_or(Failure::InvalidResponse)?;
        Some(serde_json::to_string(value).map_err(|_| Failure::InvalidResponse)?)
    } else {
        None
    };
    let body = if let Some(body) = matrix_body.as_deref() {
        Some(body)
    } else if r.endpoint == Endpoint::Claim {
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
            if body.is_some() {
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
        request = request.body(body.to_owned())
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
            let client = new_client(Duration::from_secs(
                if matches!(
                    request.endpoint,
                    Endpoint::UserExtra | Endpoint::Membership | Endpoint::PlanQuota
                ) {
                    3
                } else {
                    30
                },
            ))?;
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

#[cfg(test)]
mod entitlement_wire_tests {
    use super::*;
    #[test]
    fn membership_queries_use_official_mcode_routing_and_signed_post_body() {
        let client = new_client(Duration::from_secs(1)).unwrap();
        for (endpoint, path, body) in [
            (
                Endpoint::UserExtra,
                "/matrix/api/v1/user/get_user_extra_info",
                serde_json::json!({}),
            ),
            (
                Endpoint::Membership,
                "/matrix/api/v1/commerce/get_membership_info",
                serde_json::json!({"workspace_id":"personal-a"}),
            ),
        ] {
            let request = build_request(
                &client,
                Request {
                    endpoint,
                    region: Region::Cn,
                    access_token: Some("synthetic-access".into()),
                    real_user_id: Some("synthetic-user".into()),
                    body: Some(body.clone()),
                },
                1790000000000,
                28800,
                "linux",
                "test",
            )
            .unwrap();
            assert_eq!(request.method(), reqwest::Method::POST);
            assert_eq!(request.url().host_str(), Some("agent.minimaxi.com"));
            assert_eq!(request.url().path(), path);
            assert_eq!(request.url().query().unwrap(), "device_platform=mcode&biz_id=3&app_id=3001&version_code=22201&unix=1790000000000&timezone_offset=28800&sys_language=zh&lang=zh&device_id=0&os_name=linux&browser_name=mcode&user_id=synthetic-user&client=mcode");
            assert_eq!(
                request.body().unwrap().as_bytes().unwrap(),
                serde_json::to_vec(&body).unwrap()
            );
            let (yy, signature) = if endpoint == Endpoint::UserExtra {
                (
                    "b0cc2e36f30ab097078880f1658aeffc",
                    "baa0719f19a44df3100cf47ac059c8cb",
                )
            } else {
                (
                    "65bd5e4a071db2641b7b81453c089641",
                    "496afb41b07f84ea163421be7dbe7e55",
                )
            };
            assert_eq!(request.headers()["yy"], yy);
            assert_eq!(request.headers()["x-signature"], signature);
        }
    }
    #[test]
    fn profile_and_checkin_keep_original_methods_and_routing() {
        let client = new_client(Duration::from_secs(1)).unwrap();
        for endpoint in [Endpoint::Profile, Endpoint::Status, Endpoint::Claim] {
            let request = build_request(
                &client,
                Request {
                    endpoint,
                    region: Region::Global,
                    access_token: Some("synthetic-access".into()),
                    real_user_id: Some("synthetic-user".into()),
                    body: (endpoint == Endpoint::Claim).then(|| serde_json::json!({})),
                },
                1790000000000,
                0,
                "linux",
                "test",
            )
            .unwrap();
            assert_eq!(
                request.method(),
                if endpoint == Endpoint::Claim {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                }
            );
            assert_eq!(request.url().host_str(), Some("agent.minimax.io"));
            assert_eq!(
                request
                    .url()
                    .query_pairs()
                    .any(|(k, v)| k == "device_platform" && v == "web"),
                endpoint != Endpoint::Profile
            );
        }
    }
    #[test]
    fn plan_quota_uses_official_oauth_origins_group_and_get_without_matrix_signature() {
        let client = new_client(Duration::from_secs(1)).unwrap();
        for (region, host) in [
            (Region::Cn, "www.minimaxi.com"),
            (Region::Global, "platform.minimax.io"),
        ] {
            let request = build_request(
                &client,
                Request {
                    endpoint: Endpoint::PlanQuota,
                    region,
                    access_token: Some("synthetic-oauth".into()),
                    real_user_id: Some("synthetic-user".into()),
                    body: Some(serde_json::json!({"op_group_id":"personal-group"})),
                },
                1790000000000,
                0,
                "linux",
                "test",
            )
            .unwrap();
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(request.url().host_str(), Some(host));
            assert_eq!(
                request.url().path(),
                "/v1/api/openplatform/coding_plan/remains"
            );
            assert!(request.url().query().is_none());
            assert!(request.body().is_none());
            assert_eq!(request.headers()["authorization"], "Bearer synthetic-oauth");
            assert!(request.headers()["authorization"].is_sensitive());
            assert_eq!(request.headers()["x-group-id"], "personal-group");
            assert!(request.headers()["x-group-id"].is_sensitive());
            for header in ["yy", "x-signature", "x-timestamp"] {
                assert!(request.headers().get(header).is_none());
            }
        }
    }
    #[test]
    fn plan_quota_missing_or_invalid_personal_group_is_rejected_before_request() {
        let client = new_client(Duration::from_secs(1)).unwrap();
        for group in [
            serde_json::Value::Null,
            serde_json::json!(""),
            serde_json::json!("bad\r\nX-Evil: 1"),
            serde_json::json!("x".repeat(257)),
        ] {
            assert!(build_request(
                &client,
                Request {
                    endpoint: Endpoint::PlanQuota,
                    region: Region::Cn,
                    access_token: Some("synthetic-oauth".into()),
                    real_user_id: Some("synthetic-user".into()),
                    body: Some(serde_json::json!({"op_group_id":group}))
                },
                1790000000000,
                0,
                "linux",
                "test"
            )
            .is_err());
        }
    }
}
