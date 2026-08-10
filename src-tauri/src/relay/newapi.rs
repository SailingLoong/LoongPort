#![allow(dead_code)]

//! NewAPI 的窄 DTO 与严格响应 parser。
//!
//! 这里只承载协议形状，不负责 HTTP 请求、登录态或业务编排。所有 parser 都先验证
//! NewAPI 的 `success/data` envelope；服务端失败消息不回传，避免把敏感值带进错误文本。

use std::collections::BTreeMap;

use cookie::Cookie;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::relay::backend::{BackendKind, DetectedSite, ProbeAdapter, ProbeCandidate};

pub const PROBE_ADAPTER: ProbeAdapter = ProbeAdapter {
    candidate: ProbeCandidate {
        id: "newapi",
        path: "/api/status",
    },
    detect: detect_site,
};

fn detect_site(body: &str) -> Option<DetectedSite> {
    let status = parse_status(body).ok()?;
    Some(DetectedSite {
        backend_kind: BackendKind::NewApi,
        site_name: status.system_name,
        api_base_url: String::new(),
    })
}

#[allow(dead_code)]
fn relay_uses_newapi_backend(relay: &crate::relay::creds::Relay) -> bool {
    relay.backend_kind == BackendKind::NewApi
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    _message: String,
    data: Option<T>,
}

fn parse_envelope<T: DeserializeOwned>(
    body: &str,
    operation: &str,
    requires_data: bool,
) -> Result<Option<T>, AppError> {
    let envelope: Envelope<T> = serde_json::from_str(body)
        .map_err(|error| AppError::Config(format!("newapi {operation} 响应格式无效: {error}")))?;
    if !envelope.success {
        return Err(AppError::Config(format!("newapi {operation} 请求失败")));
    }
    if requires_data && envelope.data.is_none() {
        return Err(AppError::Config(format!(
            "newapi {operation} 响应缺少 data"
        )));
    }
    Ok(envelope.data)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub system_name: String,
    pub theme: String,
    pub register_enabled: bool,
    pub password_login_enabled: bool,
}

pub fn parse_status(body: &str) -> Result<Status, AppError> {
    let status = parse_envelope::<Status>(body, "status", true)?
        .expect("requires_data guarantees status data");
    if status.version.trim().is_empty() {
        return Err(AppError::Config("newapi status 缺少非空 version".into()));
    }
    if status.system_name.trim().is_empty() {
        return Err(AppError::Config(
            "newapi status 缺少非空 system_name".into(),
        ));
    }
    if status.theme != "default" {
        return Err(AppError::Config("newapi status theme 不是 default".into()));
    }
    Ok(status)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfAccount {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub group: String,
    pub quota: i64,
    pub used_quota: i64,
}

pub fn parse_self(body: &str) -> Result<SelfAccount, AppError> {
    let account = parse_envelope::<SelfAccount>(body, "self", true)?
        .expect("requires_data guarantees self data");
    validate_self_account(&account, "self")?;
    Ok(account)
}

fn validate_self_account(account: &SelfAccount, operation: &str) -> Result<(), AppError> {
    if account.id <= 0 {
        return Err(AppError::Config(format!(
            "newapi {operation} 响应缺少有效 user.id"
        )));
    }
    for (field, value) in [
        ("username", account.username.trim()),
        ("display_name", account.display_name.trim()),
        ("email", account.email.trim()),
        ("group", account.group.trim()),
    ] {
        if value.is_empty() {
            return Err(AppError::Config(format!(
                "newapi {operation} 响应缺少非空 user.{field}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupIdentity(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub identity: GroupIdentity,
    pub name: String,
    pub rate_multiplier: Option<f64>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct GroupWire {
    ratio: RatioWire,
    desc: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RatioWire {
    Number(f64),
    Text(String),
}

impl RatioWire {
    fn into_rate_multiplier(self) -> Result<Option<f64>, AppError> {
        match self {
            Self::Number(value) if value.is_finite() => Ok(Some(value)),
            Self::Number(_) => Err(AppError::Config("newapi groups ratio 不是有限数字".into())),
            Self::Text(value) if value == "自动" => Ok(None),
            Self::Text(_) => Err(AppError::Config(
                "newapi groups ratio 不是数字或自动".into(),
            )),
        }
    }
}

pub fn parse_groups(body: &str) -> Result<Vec<Group>, AppError> {
    let groups = parse_envelope::<BTreeMap<String, GroupWire>>(body, "groups", true)?
        .expect("requires_data guarantees groups data");
    groups
        .into_iter()
        .map(|(name, wire)| {
            Ok(Group {
                identity: GroupIdentity(name.clone()),
                name,
                rate_multiplier: wire.ratio.into_rate_multiplier()?,
                description: wire.desc,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub status: i64,
    #[serde(default)]
    pub remain_quota: i64,
    #[serde(default)]
    pub used_quota: i64,
    #[serde(default)]
    pub unlimited_quota: bool,
    #[serde(default)]
    pub expired_time: i64,
    #[serde(default)]
    pub created_time: i64,
    #[serde(default)]
    pub accessed_time: i64,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub auto_groups: Option<Vec<String>>,
    #[serde(default)]
    pub cross_group_retry: bool,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: String,
    #[serde(default)]
    pub allow_ips: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenPage {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub items: Vec<Token>,
}

pub fn parse_token_list(body: &str) -> Result<TokenPage, AppError> {
    Ok(parse_envelope::<TokenPage>(body, "token list", true)?
        .expect("requires_data guarantees token list data"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCreate;

pub fn parse_token_create(body: &str) -> Result<TokenCreate, AppError> {
    parse_envelope::<serde_json::Value>(body, "token create", false)?;
    Ok(TokenCreate)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenReveal {
    pub key: String,
}

pub fn parse_token_reveal(body: &str) -> Result<TokenReveal, AppError> {
    let reveal = parse_envelope::<TokenReveal>(body, "token reveal", true)?
        .expect("requires_data guarantees token reveal data");
    if reveal.key.is_empty() {
        return Err(AppError::Config("newapi token reveal 响应缺少 key".into()));
    }
    Ok(reveal)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenDelete;

pub fn parse_token_delete(body: &str) -> Result<TokenDelete, AppError> {
    parse_envelope::<serde_json::Value>(body, "token delete", false)?;
    Ok(TokenDelete)
}

pub struct RefreshedSession {
    pub access_token: String,
    pub access_expires_at: i64,
    pub session_id: String,
    pub account: SelfAccount,
    pub refresh_cookie: String,
}

#[derive(Debug, Deserialize)]
struct RefreshEnvelope {
    access_token: String,
    access_expires_at: i64,
    user: SelfAccount,
    session: RefreshSessionWire,
}

#[derive(Debug, Deserialize)]
struct RefreshSessionWire {
    sid: String,
}

pub async fn refresh_session(
    site_origin: &str,
    refresh_cookie: &str,
    expected_sid: Option<&str>,
) -> Result<RefreshedSession, AppError> {
    let site_origin = site_origin.trim().trim_end_matches('/');
    if site_origin.is_empty() {
        return Err(AppError::InvalidInput("newapi site_origin 不能为空".into()));
    }
    if refresh_cookie.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "newapi refresh cookie 不能为空".into(),
        ));
    }

    let client = crate::relay::api::build_client()?;
    let mut request = client
        .post(format!("{site_origin}/api/user/auth/refresh"))
        .header("Origin", site_origin)
        .header("Cookie", format!("new_api_refresh={refresh_cookie}"));
    let expected_sid = expected_sid.unwrap_or("").trim();
    if !expected_sid.is_empty() {
        request = request.header("X-Auth-Session", expected_sid);
    }

    let response = request.send().await.map_err(|error| {
        AppError::Config(format!(
            "newapi refresh 请求失败: {}",
            describe_send_error(&error)
        ))
    })?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Config(format!("newapi refresh 读响应出错: {error}")))?;
    if !status.is_success() {
        return Err(AppError::Config(format!(
            "newapi refresh 请求失败: HTTP {}",
            status.as_u16()
        )));
    }

    let refreshed = parse_envelope::<RefreshEnvelope>(&body, "refresh", true)?
        .expect("requires_data guarantees refresh data");
    if refreshed.access_token.trim().is_empty() {
        return Err(AppError::Config(
            "newapi refresh 响应缺少非空 access_token".into(),
        ));
    }
    if refreshed.access_expires_at <= 0 {
        return Err(AppError::Config(
            "newapi refresh 响应缺少有效 access_expires_at".into(),
        ));
    }
    if refreshed.session.sid.trim().is_empty() {
        return Err(AppError::Config(
            "newapi refresh 响应缺少非空 session.sid".into(),
        ));
    }
    validate_self_account(&refreshed.user, "refresh")?;
    let rotated_cookie = extract_rotated_refresh_cookie(&headers)?;

    Ok(RefreshedSession {
        access_token: refreshed.access_token,
        access_expires_at: refreshed.access_expires_at,
        session_id: refreshed.session.sid,
        account: refreshed.user,
        refresh_cookie: rotated_cookie,
    })
}

fn extract_rotated_refresh_cookie(
    headers: &reqwest::header::HeaderMap,
) -> Result<String, AppError> {
    for header in headers.get_all(reqwest::header::SET_COOKIE) {
        let raw = header
            .to_str()
            .map_err(|_| AppError::Config("newapi refresh Set-Cookie 不是合法 ASCII".into()))?;
        let cookie = Cookie::parse(raw.to_owned())
            .map_err(|_| AppError::Config("newapi refresh Set-Cookie 格式无效".into()))?;
        if cookie.name() == "new_api_refresh" {
            if cookie.value().trim().is_empty() {
                return Err(AppError::Config(
                    "newapi refresh Set-Cookie 缺少非空 new_api_refresh".into(),
                ));
            }
            return Ok(cookie.value().to_string());
        }
    }
    Err(AppError::Config(
        "newapi refresh 响应缺少 rotated new_api_refresh cookie".into(),
    ))
}

fn describe_send_error(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连不上服务器"
    } else if error.is_request() {
        "请求发送失败"
    } else {
        "网络错误"
    };

    let mut output = format!("{kind}（{error}）");
    let mut source = std::error::Error::source(error);
    while let Some(next) = source {
        output.push_str(&format!(" cause: {next}"));
        source = next.source();
    }
    output
}

const TOKEN_PAGE_SIZE: i64 = 100;
const TOKEN_PAGE_LIMIT: i64 = 100;

pub struct NewApiClient {
    site_origin: String,
    access_token: String,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct CreateTokenPayload<'a> {
    name: &'a str,
    remain_quota: i64,
    expired_time: i64,
    unlimited_quota: bool,
    model_limits_enabled: bool,
    model_limits: &'a str,
    allow_ips: &'a str,
    group: &'a str,
    auto_groups: [String; 0],
    cross_group_retry: bool,
}

impl NewApiClient {
    pub fn new(site_origin: &str, access_token: &str) -> Result<Self, AppError> {
        let site_origin = site_origin.trim().trim_end_matches('/').to_string();
        if site_origin.is_empty() {
            return Err(AppError::InvalidInput("newapi site_origin 不能为空".into()));
        }
        if access_token.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "newapi access token 不能为空".into(),
            ));
        }
        Ok(Self {
            site_origin,
            access_token: access_token.to_string(),
            http: crate::relay::api::build_client()?,
        })
    }

    pub async fn account(&self) -> Result<SelfAccount, AppError> {
        let body = self
            .send(self.request(reqwest::Method::GET, "/api/user/self"), "self")
            .await?;
        parse_self(&body)
    }

    pub async fn groups(&self) -> Result<Vec<Group>, AppError> {
        let body = self
            .send(
                self.request(reqwest::Method::GET, "/api/user/self/groups"),
                "groups",
            )
            .await?;
        parse_groups(&body)
    }

    pub async fn list_tokens(&self) -> Result<Vec<Token>, AppError> {
        let mut items = Vec::new();
        for page in 1..=TOKEN_PAGE_LIMIT {
            let body = self
                .send(
                    self.request(
                        reqwest::Method::GET,
                        &format!("/api/token/?p={page}&size={TOKEN_PAGE_SIZE}"),
                    ),
                    "token list",
                )
                .await?;
            let token_page = parse_token_list(&body)?;
            if token_page.items.is_empty() {
                return Ok(items);
            }
            let total = token_page.total.max(0) as usize;
            items.extend(token_page.items);
            if total > 0 && items.len() >= total {
                return Ok(items);
            }
        }
        Err(AppError::Config("newapi token list 超过分页上限".into()))
    }

    pub async fn create_token(&self, managed_name: &str, group_name: &str) -> Result<(), AppError> {
        let body = self
            .send(
                self.request(reqwest::Method::POST, "/api/token/")
                    .json(&CreateTokenPayload {
                        name: managed_name,
                        remain_quota: 0,
                        expired_time: -1,
                        unlimited_quota: true,
                        model_limits_enabled: false,
                        model_limits: "",
                        allow_ips: "",
                        group: group_name,
                        auto_groups: [],
                        cross_group_retry: false,
                    }),
                "token create",
            )
            .await?;
        parse_token_create(&body)?;
        Ok(())
    }

    pub async fn reveal_token(&self, token_id: i64) -> Result<String, AppError> {
        let body = self
            .send(
                self.request(reqwest::Method::POST, &format!("/api/token/{token_id}/key")),
                "token reveal",
            )
            .await?;
        Ok(parse_token_reveal(&body)?.key)
    }

    pub async fn delete_token(&self, token_id: i64) -> Result<(), AppError> {
        let body = self
            .send(
                self.request(reqwest::Method::DELETE, &format!("/api/token/{token_id}")),
                "token delete",
            )
            .await?;
        parse_token_delete(&body)?;
        Ok(())
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.site_origin, path))
            .bearer_auth(&self.access_token)
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<String, AppError> {
        let response = request.send().await.map_err(|error| {
            AppError::Config(format!(
                "newapi {operation} 请求失败: {}",
                describe_send_error(&error)
            ))
        })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| AppError::Config(format!("newapi {operation} 读响应出错: {error}")))?;
        if !status.is_success() {
            return Err(AppError::Config(format!(
                "newapi {operation} 请求失败: HTTP {}",
                status.as_u16()
            )));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Arc;

    use bytes::Bytes;
    use http::{Response, StatusCode};
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tokio::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedRequest {
        method: String,
        path_and_query: String,
        headers: BTreeMap<String, String>,
        body: String,
    }

    impl RecordedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .get(&name.to_ascii_lowercase())
                .map(String::as_str)
        }
    }

    #[derive(Debug, Clone)]
    struct TestResponse {
        status: StatusCode,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl TestResponse {
        fn json(body: &str) -> Self {
            Self {
                status: StatusCode::OK,
                headers: vec![("content-type".into(), "application/json".into())],
                body: body.into(),
            }
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.into(), value.into()));
            self
        }
    }

    struct TestServer {
        origin: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    impl TestServer {
        async fn spawn(responses: Vec<TestResponse>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test listener");
            let origin = format!("http://{}", listener.local_addr().expect("local addr"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
            let requests_for_task = Arc::clone(&requests);
            let responses_for_task = Arc::clone(&responses);

            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let requests = Arc::clone(&requests_for_task);
                    let responses = Arc::clone(&responses_for_task);
                    tokio::spawn(async move {
                        let service = service_fn(move |request: hyper::Request<Incoming>| {
                            let requests = Arc::clone(&requests);
                            let responses = Arc::clone(&responses);
                            async move {
                                let (parts, body) = request.into_parts();
                                let body = body.collect().await.expect("collect body").to_bytes();
                                let headers = parts
                                    .headers
                                    .iter()
                                    .map(|(name, value)| {
                                        (
                                            name.as_str().to_ascii_lowercase(),
                                            value
                                                .to_str()
                                                .expect("header should be ASCII")
                                                .to_string(),
                                        )
                                    })
                                    .collect();
                                requests.lock().await.push(RecordedRequest {
                                    method: parts.method.to_string(),
                                    path_and_query: parts
                                        .uri
                                        .path_and_query()
                                        .map(|value| value.as_str().to_string())
                                        .unwrap_or_else(|| parts.uri.path().to_string()),
                                    headers,
                                    body: String::from_utf8(body.to_vec())
                                        .expect("body should be valid UTF-8"),
                                });

                                let response =
                                    responses.lock().await.pop_front().unwrap_or_else(|| {
                                        TestResponse {
                                            status: StatusCode::INTERNAL_SERVER_ERROR,
                                            headers: vec![],
                                            body: "unexpected request".into(),
                                        }
                                    });
                                let mut builder = Response::builder().status(response.status);
                                for (name, value) in &response.headers {
                                    builder = builder.header(name, value);
                                }
                                Ok::<_, hyper::Error>(
                                    builder
                                        .body(Full::new(Bytes::from(response.body)))
                                        .expect("build response"),
                                )
                            }
                        });
                        let _ = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });

            Self { origin, requests }
        }

        async fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().await.clone()
        }
    }

    #[tokio::test]
    async fn refresh_sends_same_origin_headers_and_rotates_cookie() {
        let server = TestServer::spawn(vec![TestResponse::json(
            r#"{
                    "success": true,
                    "message": "",
                    "data": {
                        "access_token": "access-123",
                        "token_type": "Bearer",
                        "access_expires_at": 1700000001,
                        "user": {
                            "id": 7,
                            "username": "alice",
                            "display_name": "Alice",
                            "email": "a@example.com",
                            "group": "vip",
                            "quota": 12345,
                            "used_quota": 678
                        },
                        "session": {
                            "sid": "sid-123",
                            "current": true,
                            "login_method": "passkey",
                            "ip": "127.0.0.1",
                            "user_agent": "Safari",
                            "created_at": 1700000000,
                            "last_active_at": 1700000001,
                            "expires_at": 1700003600
                        }
                    }
                }"#,
        )
        .with_header(
            "set-cookie",
            "new_api_refresh=sid-123.rotated-secret; Path=/; HttpOnly; SameSite=Lax",
        )])
        .await;

        let refreshed = refresh_session(&server.origin, "sid-123.previous-secret", Some("   "))
            .await
            .unwrap();

        assert_eq!(refreshed.access_token, "access-123");
        assert_eq!(refreshed.access_expires_at, 1_700_000_001);
        assert_eq!(refreshed.session_id, "sid-123");
        assert_eq!(refreshed.refresh_cookie, "sid-123.rotated-secret");
        assert_eq!(refreshed.account.username, "alice");

        let requests = server.requests().await;
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path_and_query, "/api/user/auth/refresh");
        assert_eq!(request.header("origin"), Some(server.origin.as_str()));
        assert_eq!(
            request.header("cookie"),
            Some("new_api_refresh=sid-123.previous-secret")
        );
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("x-auth-session"), None);
        assert_eq!(request.body, "");
    }

    #[tokio::test]
    async fn refresh_sends_x_auth_session_when_provided() {
        let server = TestServer::spawn(vec![TestResponse::json(
            r#"{
                    "success": true,
                    "message": "",
                    "data": {
                        "access_token": "access-456",
                        "token_type": "Bearer",
                        "access_expires_at": 1700000002,
                        "user": {
                            "id": 8,
                            "username": "bob",
                            "display_name": "Bob",
                            "email": "b@example.com",
                            "group": "default",
                            "quota": 1,
                            "used_quota": 0
                        },
                        "session": {
                            "sid": "sid-provided",
                            "current": true,
                            "login_method": "oauth",
                            "ip": "127.0.0.1",
                            "user_agent": "Safari",
                            "created_at": 1700000000,
                            "last_active_at": 1700000001,
                            "expires_at": 1700003600
                        }
                    }
                }"#,
        )
        .with_header(
            "set-cookie",
            "new_api_refresh=sid-provided.rotated-secret; Path=/; HttpOnly",
        )])
        .await;

        refresh_session(
            &server.origin,
            "sid-provided.previous-secret",
            Some("sid-provided"),
        )
        .await
        .unwrap();

        let requests = server.requests().await;
        assert_eq!(requests[0].header("x-auth-session"), Some("sid-provided"));
    }

    #[tokio::test]
    async fn failed_refresh_reports_http_status_before_cookie_rotation_requirement() {
        let request_secret = "sid-401.previous-secret";
        let response_secret = "sid-401.response-secret";
        let server = TestServer::spawn(vec![TestResponse {
            status: StatusCode::UNAUTHORIZED,
            headers: vec![("content-type".into(), "application/json".into())],
            body: format!(r#"{{"success":false,"message":"bad {response_secret}"}}"#),
        }])
        .await;

        let error = match refresh_session(&server.origin, request_secret, None).await {
            Ok(_) => panic!("refresh should fail with HTTP status context"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("HTTP 401"), "{error}");
        assert!(!error.contains("rotated new_api_refresh cookie"), "{error}");
        assert!(!error.contains("previous-secret"), "{error}");
        assert!(!error.contains("response-secret"), "{error}");
    }

    #[tokio::test]
    async fn successful_refresh_requires_rotated_refresh_cookie() {
        let server = TestServer::spawn(vec![TestResponse::json(
            r#"{
                "success": true,
                "message": "",
                "data": {
                    "access_token": "access-789",
                    "token_type": "Bearer",
                    "access_expires_at": 1700000003,
                    "user": {
                        "id": 9,
                        "username": "carol",
                        "display_name": "Carol",
                        "email": "c@example.com",
                        "group": "vip",
                        "quota": 5,
                        "used_quota": 1
                    },
                    "session": {
                        "sid": "sid-789",
                        "current": true,
                        "login_method": "oauth",
                        "ip": "127.0.0.1",
                        "user_agent": "Safari",
                        "created_at": 1700000000,
                        "last_active_at": 1700000001,
                        "expires_at": 1700003600
                    }
                }
            }"#,
        )])
        .await;

        let error = match refresh_session(&server.origin, "sid-789.previous-secret", None).await {
            Ok(_) => panic!("refresh should fail without rotated cookie"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("rotated new_api_refresh cookie"), "{error}");
        assert!(!error.contains("previous-secret"), "{error}");
    }

    #[tokio::test]
    async fn malformed_and_failed_refresh_responses_do_not_leak_secrets() {
        let request_secret = "sid-secret.request-secret";
        let response_secret = "sid-secret.response-secret";
        let body_secret = "body-secret";

        let failed = TestServer::spawn(vec![TestResponse::json(&format!(
            r#"{{
                    "success": false,
                    "message": "bad {body_secret}",
                    "data": null
                }}"#
        ))
        .with_header(
            "set-cookie",
            &format!("new_api_refresh={response_secret}; Path=/; HttpOnly"),
        )])
        .await;
        let failed_error = match refresh_session(&failed.origin, request_secret, None).await {
            Ok(_) => panic!("refresh should fail on success:false envelope"),
            Err(error) => error.to_string(),
        };
        assert!(!failed_error.contains("request-secret"), "{failed_error}");
        assert!(!failed_error.contains(response_secret), "{failed_error}");
        assert!(!failed_error.contains(body_secret), "{failed_error}");

        let malformed = TestServer::spawn(vec![TestResponse::json(
            r#"{
                    "success": true,
                    "message": "",
                    "data": {
                        "access_token": "",
                        "token_type": "Bearer",
                        "access_expires_at": 1700000004,
                        "user": {
                            "id": 10,
                            "username": "dave",
                            "display_name": "Dave",
                            "email": "d@example.com",
                            "group": "vip",
                            "quota": 5,
                            "used_quota": 1
                        },
                        "session": {
                            "sid": "sid-secret",
                            "current": true,
                            "login_method": "oauth",
                            "ip": "127.0.0.1",
                            "user_agent": "Safari",
                            "created_at": 1700000000,
                            "last_active_at": 1700000001,
                            "expires_at": 1700003600
                        }
                    }
                }"#,
        )
        .with_header(
            "set-cookie",
            &format!("new_api_refresh={response_secret}; Path=/; HttpOnly"),
        )])
        .await;
        let malformed_error = match refresh_session(&malformed.origin, request_secret, None).await {
            Ok(_) => panic!("refresh should fail on malformed auth payload"),
            Err(error) => error.to_string(),
        };
        assert!(
            !malformed_error.contains("request-secret"),
            "{malformed_error}"
        );
        assert!(
            !malformed_error.contains(response_secret),
            "{malformed_error}"
        );
    }

    #[tokio::test]
    async fn authenticated_account_and_groups_calls_use_bearer_header() {
        let server = TestServer::spawn(vec![
            TestResponse::json(
                r#"{"success":true,"message":"","data":{
                    "id":11,"username":"erin","display_name":"Erin","email":"e@example.com",
                    "group":"vip","quota":12,"used_quota":3
                }}"#,
            ),
            TestResponse::json(
                r#"{"success":true,"message":"","data":{
                    "vip":{"ratio":1.5,"desc":"paid"},
                    "auto":{"ratio":"自动","desc":"automatic"}
                }}"#,
            ),
        ])
        .await;
        let client = NewApiClient::new(&server.origin, "access-secret").unwrap();

        let account = client.account().await.unwrap();
        let groups = client.groups().await.unwrap();

        assert_eq!(account.username, "erin");
        assert_eq!(groups.len(), 2);

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path_and_query, "/api/user/self");
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer access-secret")
        );
        assert_eq!(requests[1].method, "GET");
        assert_eq!(requests[1].path_and_query, "/api/user/self/groups");
        assert_eq!(
            requests[1].header("authorization"),
            Some("Bearer access-secret")
        );
    }

    #[tokio::test]
    async fn token_listing_starts_at_page_one_and_stops_on_empty_page() {
        let server = TestServer::spawn(vec![
            TestResponse::json(
                r#"{"success":true,"message":"","data":{
                    "page":1,"page_size":100,"total":250,
                    "items":[
                        {"id":1,"name":"managed-1","key":"sk-***","status":1,"group":"vip"},
                        {"id":2,"name":"managed-2","key":"sk-***","status":1,"group":"vip"}
                    ]
                }}"#,
            ),
            TestResponse::json(
                r#"{"success":true,"message":"","data":{
                    "page":2,"page_size":100,"total":250,
                    "items":[
                        {"id":3,"name":"managed-3","key":"sk-***","status":1,"group":"vip"}
                    ]
                }}"#,
            ),
            TestResponse::json(
                r#"{"success":true,"message":"","data":{
                    "page":3,"page_size":100,"total":250,"items":[]
                }}"#,
            ),
        ])
        .await;
        let client = NewApiClient::new(&server.origin, "access-secret").unwrap();

        let tokens = client.list_tokens().await.unwrap();

        assert_eq!(
            tokens.iter().map(|token| token.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let requests = server.requests().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].path_and_query, "/api/token/?p=1&size=100");
        assert_eq!(requests[1].path_and_query, "/api/token/?p=2&size=100");
        assert_eq!(requests[2].path_and_query, "/api/token/?p=3&size=100");
    }

    #[tokio::test]
    async fn create_token_sends_the_upstream_unlimited_payload() {
        let server =
            TestServer::spawn(vec![TestResponse::json(r#"{"success":true,"message":""}"#)]).await;
        let client = NewApiClient::new(&server.origin, "access-secret").unwrap();

        client
            .create_token("LoongPort/device/vip", "vip")
            .await
            .unwrap();

        let requests = server.requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path_and_query, "/api/token/");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&requests[0].body).unwrap(),
            serde_json::json!({
                "name": "LoongPort/device/vip",
                "remain_quota": 0,
                "expired_time": -1,
                "unlimited_quota": true,
                "model_limits_enabled": false,
                "model_limits": "",
                "allow_ips": "",
                "group": "vip",
                "auto_groups": [],
                "cross_group_retry": false
            })
        );
    }

    #[tokio::test]
    async fn reveal_and_delete_use_the_expected_endpoints() {
        let server = TestServer::spawn(vec![
            TestResponse::json(r#"{"success":true,"message":"","data":{"key":"sk-full-secret"}}"#),
            TestResponse::json(r#"{"success":true,"message":""}"#),
        ])
        .await;
        let client = NewApiClient::new(&server.origin, "access-secret").unwrap();

        assert_eq!(client.reveal_token(42).await.unwrap(), "sk-full-secret");
        client.delete_token(42).await.unwrap();

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path_and_query, "/api/token/42/key");
        assert_eq!(requests[1].method, "DELETE");
        assert_eq!(requests[1].path_and_query, "/api/token/42");
    }

    #[tokio::test]
    async fn authenticated_failures_do_not_leak_response_or_access_secrets() {
        let access_secret = "access-secret";
        let http_secret = "http-secret";
        let envelope_secret = "envelope-secret";

        let http_failure = TestServer::spawn(vec![TestResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            headers: vec![("content-type".into(), "application/json".into())],
            body: format!(r#"{{"detail":"{http_secret}"}}"#),
        }])
        .await;
        let http_client = NewApiClient::new(&http_failure.origin, access_secret).unwrap();
        let http_error = match http_client.account().await {
            Ok(_) => panic!("account should fail on HTTP 500"),
            Err(error) => error.to_string(),
        };
        assert!(!http_error.contains(access_secret), "{http_error}");
        assert!(!http_error.contains(http_secret), "{http_error}");

        let envelope_failure = TestServer::spawn(vec![TestResponse::json(&format!(
            r#"{{"success":false,"message":"bad {envelope_secret}"}}"#
        ))])
        .await;
        let envelope_client = NewApiClient::new(&envelope_failure.origin, access_secret).unwrap();
        let envelope_error = match envelope_client.groups().await {
            Ok(_) => panic!("groups should fail on success:false envelope"),
            Err(error) => error.to_string(),
        };
        assert!(!envelope_error.contains(access_secret), "{envelope_error}");
        assert!(
            !envelope_error.contains(envelope_secret),
            "{envelope_error}"
        );
    }

    #[test]
    fn status_parser_requires_success_envelope_and_stable_fields() {
        let status = parse_status(
            r#"{
                "success": true,
                "message": "",
                "data": {
                    "version": "1.2.3",
                    "system_name": "New API",
                    "theme": "default",
                    "register_enabled": true,
                    "password_login_enabled": false
                }
            }"#,
        )
        .unwrap();
        assert_eq!(status.version, "1.2.3");
        assert_eq!(status.system_name, "New API");
        assert!(!status.password_login_enabled);

        for body in [
            r#"{"success":false,"message":"nope","data":{}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":"New API","theme":"default","register_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":"New API","theme":"classic","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"","system_name":"New API","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":"1","system_name":" ","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
            r#"{"success":true,"data":{"version":1,"system_name":"New API","theme":"default","register_enabled":true,"password_login_enabled":true}}"#,
        ] {
            assert!(
                parse_status(body).is_err(),
                "accepted invalid status: {body}"
            );
        }
    }

    #[test]
    fn self_parser_reads_identity_and_quota_fields() {
        let account = parse_self(
            r#"{"success":true,"message":"","data":{
                "id":7,"username":"alice","display_name":"Alice","email":"a@example.com",
                "group":"vip","quota":12345,"used_quota":678
            }}"#,
        )
        .unwrap();
        assert_eq!(account.id, 7);
        assert_eq!(account.username, "alice");
        assert_eq!(account.display_name, "Alice");
        assert_eq!(account.email, "a@example.com");
        assert_eq!(account.group, "vip");
        assert_eq!(account.quota, 12345);
        assert_eq!(account.used_quota, 678);
    }

    #[test]
    fn groups_parser_supports_numeric_and_auto_ratios_without_losing_identity() {
        let groups = parse_groups(
            r#"{"success":true,"message":"","data":{
                "vip / 特殊": {"ratio": 0.75, "desc":"paid"},
                "自动": {"ratio":"自动", "desc":"automatic"}
            }}"#,
        )
        .unwrap();
        let special = groups
            .iter()
            .find(|group| group.name == "vip / 特殊")
            .unwrap();
        assert_eq!(special.identity.0, "vip / 特殊");
        assert_eq!(special.rate_multiplier, Some(0.75));
        let automatic = groups.iter().find(|group| group.name == "自动").unwrap();
        assert_eq!(automatic.identity.0, "自动");
        assert_eq!(automatic.rate_multiplier, None);
    }

    #[test]
    fn token_parsers_cover_list_create_reveal_and_delete() {
        let page = parse_token_list(
            r#"{"success":true,"message":"","data":{
                "page":1,"page_size":10,"total":1,
                "items":[{"id":9,"name":"relay","key":"sk-****","status":1}]
            }}"#,
        )
        .unwrap();
        assert_eq!(page.items[0].id, 9);
        assert_eq!(page.items[0].key, "sk-****");

        assert!(parse_token_create(r#"{"success":true,"message":""}"#).is_ok());
        assert!(parse_token_delete(r#"{"success":true,"message":""}"#).is_ok());
        assert_eq!(
            parse_token_reveal(r#"{"success":true,"message":"","data":{"key":"sk-full-secret"}}"#,)
                .unwrap()
                .key,
            "sk-full-secret"
        );
    }

    #[test]
    fn token_parsers_reject_failed_envelopes_without_leaking_reveal_key() {
        let error = parse_token_reveal(
            r#"{"success":false,"message":"bad sk-full-secret","data":{"key":"sk-full-secret"}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("sk-full-secret"));
        assert!(parse_token_list(r#"{"success":false,"message":"nope"}"#).is_err());
        assert!(parse_token_create(r#"{"success":false,"message":"nope"}"#).is_err());
        assert!(parse_token_delete(r#"{"success":false,"message":"nope"}"#).is_err());
    }
}
