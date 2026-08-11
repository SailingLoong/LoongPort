//! 中转站协议适配器的共享契约。
//!
//! 协议模块拥有 endpoint、wire DTO 和 detector；discovery 只遍历这里的窄描述符，
//! 不携带任何协议专属响应类型。

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::relay::{api, creds, newapi};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Sub2Api,
    NewApi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSite {
    pub backend_kind: BackendKind,
    pub site_name: String,
    pub api_base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeCandidate {
    pub id: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Copy)]
pub struct ProbeAdapter {
    pub candidate: ProbeCandidate,
    pub detect: fn(&str) -> Option<DetectedSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAccount {
    pub id: i64,
    pub label: String,
    pub login_identifier: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBalance {
    pub balance: f64,
    pub frozen_balance: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshedSession {
    pub auth_token: String,
    pub refresh_credential: Option<String>,
    pub token_expires_at: Option<i64>,
    pub account: Option<RuntimeAccount>,
}

pub fn is_confirmed_auth_failure(error: &AppError) -> bool {
    match error {
        AppError::Config(message)
        | AppError::InvalidInput(message)
        | AppError::Message(message) => {
            message.contains("登录态已失效") || message.contains("请重新登录")
        }
        _ => false,
    }
}

pub enum RuntimeBackend<'a> {
    Sub2Api { relay: &'a creds::Relay },
    NewApi { relay: &'a creds::Relay },
}

impl<'a> RuntimeBackend<'a> {
    pub fn for_relay(relay: &'a creds::Relay) -> Self {
        match relay.backend_kind {
            BackendKind::Sub2Api => Self::Sub2Api { relay },
            BackendKind::NewApi => Self::NewApi { relay },
        }
    }

    pub async fn account(&self) -> Result<RuntimeAccount, AppError> {
        match self {
            Self::Sub2Api { relay } => {
                let account =
                    api::Client::new(&relay.site_origin, &relay.auth_token, relay.account_id)?
                        .account()
                        .await?;
                Ok(RuntimeAccount {
                    id: account.id,
                    label: account.display_name(),
                    login_identifier: account.email,
                })
            }
            Self::NewApi { relay } => {
                let account = newapi::NewApiClient::new(&relay.site_origin, &relay.auth_token)?
                    .account()
                    .await?;
                Ok(newapi_runtime_account(&account))
            }
        }
    }

    pub async fn balance(&self) -> Result<RuntimeBalance, AppError> {
        match self {
            Self::Sub2Api { relay } => {
                let balance =
                    api::Client::new(&relay.site_origin, &relay.auth_token, relay.account_id)?
                        .balance()
                        .await?;
                Ok(RuntimeBalance {
                    balance: balance.balance,
                    frozen_balance: balance.frozen_balance,
                })
            }
            Self::NewApi { relay } => {
                let account = newapi::NewApiClient::new(&relay.site_origin, &relay.auth_token)?
                    .account()
                    .await?;
                let status = newapi::fetch_status(&relay.site_origin).await?;
                let quota_per_unit = status.quota_per_unit.ok_or_else(|| {
                    AppError::Config("newapi status 缺少 quota_per_unit，无法换算余额".into())
                })?;
                if !quota_per_unit.is_finite() || quota_per_unit <= 0.0 {
                    return Err(AppError::Config(
                        "newapi status quota_per_unit 必须是正数".into(),
                    ));
                }
                Ok(RuntimeBalance {
                    balance: account.quota as f64 / quota_per_unit,
                    frozen_balance: 0.0,
                })
            }
        }
    }

    pub async fn refresh_session(
        &self,
        refresh_credential: Option<&str>,
    ) -> Result<RefreshedSession, AppError> {
        let refresh_credential = refresh_credential
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Config("登录已过期，请重新登录".into()))?;

        match self {
            Self::Sub2Api { relay } => {
                let refreshed = api::refresh_token(&relay.site_origin, refresh_credential).await?;
                Ok(RefreshedSession {
                    auth_token: refreshed.auth_token,
                    refresh_credential: refreshed.refresh_token,
                    token_expires_at: refreshed.token_expires_at,
                    account: None,
                })
            }
            Self::NewApi { relay } => {
                let refreshed =
                    newapi::refresh_session(&relay.site_origin, refresh_credential, None).await?;
                Ok(RefreshedSession {
                    auth_token: refreshed.access_token,
                    refresh_credential: Some(refreshed.refresh_cookie),
                    token_expires_at: Some(refreshed.access_expires_at),
                    account: Some(newapi_runtime_account(&refreshed.account)),
                })
            }
        }
    }
}

pub fn newapi_runtime_account(account: &newapi::SelfAccount) -> RuntimeAccount {
    RuntimeAccount {
        id: account.id,
        label: first_nonblank(&[&account.display_name, &account.username, &account.email]),
        login_identifier: account.username.clone(),
    }
}

fn first_nonblank(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        http::{header, HeaderMap},
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;

    use super::*;
    use crate::relay::creds::Relay;

    fn relay(origin: &str, backend_kind: BackendKind) -> Relay {
        Relay {
            id: 7,
            site_origin: origin.to_string(),
            site_name: "Test relay".into(),
            backend_kind,
            api_base_url: origin.to_string(),
            account_id: Some(42),
            account_label: "Old label".into(),
            login_identifier: "old-login".into(),
            auth_token: "access-token".into(),
            refresh_token: Some("stored-refresh".into()),
            token_expires_at: Some(1),
            sort_index: 0,
        }
    }

    async fn spawn(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (origin, task)
    }

    #[tokio::test]
    async fn sub2api_account_and_balance_keep_existing_semantics_through_dispatcher() {
        let app = Router::new().route(
            "/api/v1/user/profile",
            get(|| async {
                Json(json!({
                    "code": 0,
                    "message": "success",
                    "data": {
                        "id": 42,
                        "username": "Sub User",
                        "email": "sub@example.com",
                        "balance": 12.5,
                        "frozen_balance": 1.25
                    }
                }))
            }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::Sub2Api);
        let backend = RuntimeBackend::for_relay(&relay);

        let account = backend.account().await.unwrap();
        let balance = backend.balance().await.unwrap();

        assert_eq!(account.id, 42);
        assert_eq!(account.label, "Sub User");
        assert_eq!(account.login_identifier, "sub@example.com");
        assert_eq!(balance.balance, 12.5);
        assert_eq!(balance.frozen_balance, 1.25);
        server.abort();
    }

    #[tokio::test]
    async fn newapi_account_uses_display_name_for_label_and_username_for_login() {
        let app = Router::new().route(
            "/api/user/self",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "id": 84,
                        "username": "newapi-login",
                        "display_name": "NewAPI Display",
                        "email": "newapi@example.com",
                        "group": "default",
                        "quota": 750000,
                        "used_quota": 4000000
                    }
                }))
            }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let account = RuntimeBackend::for_relay(&relay).account().await.unwrap();

        assert_eq!(account.id, 84);
        assert_eq!(account.label, "NewAPI Display");
        assert_eq!(account.login_identifier, "newapi-login");
        server.abort();
    }

    #[tokio::test]
    async fn newapi_account_label_falls_back_without_changing_login_identifier() {
        let app = Router::new().route(
            "/api/user/self",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "id": 84,
                        "username": "fallback-login",
                        "display_name": "",
                        "email": "fallback@example.com",
                        "group": "default",
                        "quota": 0,
                        "used_quota": 0
                    }
                }))
            }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let account = RuntimeBackend::for_relay(&relay).account().await.unwrap();

        assert_eq!(account.label, "fallback-login");
        assert_eq!(account.login_identifier, "fallback-login");
        server.abort();
    }

    #[tokio::test]
    async fn newapi_balance_converts_remaining_quota_without_subtracting_used_quota() {
        let app = Router::new()
            .route(
                "/api/user/self",
                get(|| async {
                    Json(json!({
                        "success": true,
                        "data": {
                            "id": 84,
                            "username": "quota-user",
                            "display_name": "Quota User",
                            "email": "quota@example.com",
                            "group": "default",
                            "quota": 1500000,
                            "used_quota": 9000000
                        }
                    }))
                }),
            )
            .route(
                "/api/status",
                get(|| async {
                    Json(json!({
                        "success": true,
                        "data": {
                            "version": "1.0.0",
                            "system_name": "Relay",
                            "theme": "default",
                            "register_enabled": true,
                            "password_login_enabled": true,
                            "quota_per_unit": 1000000.0
                        }
                    }))
                }),
            );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let balance = RuntimeBackend::for_relay(&relay).balance().await.unwrap();

        assert_eq!(balance.balance, 1.5);
        assert_eq!(balance.frozen_balance, 0.0);
        server.abort();
    }

    #[tokio::test]
    async fn newapi_public_status_401_is_not_a_confirmed_auth_failure() {
        let app = Router::new()
            .route(
                "/api/user/self",
                get(|| async {
                    Json(json!({
                        "success": true,
                        "data": {
                            "id": 84,
                            "username": "quota-user",
                            "display_name": "Quota User",
                            "email": "quota@example.com",
                            "group": "default",
                            "quota": 1500000,
                            "used_quota": 9000000
                        }
                    }))
                }),
            )
            .route(
                "/api/status",
                get(|| async { (axum::http::StatusCode::UNAUTHORIZED, "") }),
            );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let error = RuntimeBackend::for_relay(&relay)
            .balance()
            .await
            .unwrap_err();

        assert!(!is_confirmed_auth_failure(&error), "{error}");
        server.abort();
    }

    #[tokio::test]
    async fn newapi_authenticated_self_401_is_a_confirmed_auth_failure() {
        let app = Router::new().route(
            "/api/user/self",
            get(|| async { (axum::http::StatusCode::UNAUTHORIZED, "") }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let error = RuntimeBackend::for_relay(&relay)
            .account()
            .await
            .unwrap_err();

        assert!(is_confirmed_auth_failure(&error), "{error}");
        server.abort();
    }

    #[tokio::test]
    async fn newapi_refresh_uses_stored_cookie_and_returns_rotated_cookie() {
        let seen_cookie = Arc::new(Mutex::new(None::<String>));
        let seen_cookie_for_route = seen_cookie.clone();
        let app = Router::new().route(
            "/api/user/auth/refresh",
            post(move |headers: HeaderMap| {
                let seen_cookie = seen_cookie_for_route.clone();
                async move {
                    *seen_cookie.lock().unwrap() = headers
                        .get(header::COOKIE)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    (
                        [(
                            header::SET_COOKIE,
                            "new_api_refresh=rotated-cookie; HttpOnly",
                        )],
                        Json(json!({
                            "success": true,
                            "data": {
                                "access_token": "new-access",
                                "access_expires_at": 1900000000,
                                "user": {
                                    "id": 84,
                                    "username": "refresh-login",
                                    "display_name": "Refresh User",
                                    "email": "refresh@example.com",
                                    "group": "default",
                                    "quota": 500000,
                                    "used_quota": 0
                                },
                                "session": { "sid": "session-2" }
                            }
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let refreshed = RuntimeBackend::for_relay(&relay)
            .refresh_session(relay.refresh_token.as_deref())
            .await
            .unwrap();

        assert_eq!(
            seen_cookie.lock().unwrap().as_deref(),
            Some("new_api_refresh=stored-refresh")
        );
        assert_eq!(refreshed.auth_token, "new-access");
        assert_eq!(
            refreshed.refresh_credential.as_deref(),
            Some("rotated-cookie")
        );
        assert_eq!(refreshed.token_expires_at, Some(1_900_000_000));
        assert_eq!(refreshed.account.unwrap().login_identifier, "refresh-login");
        server.abort();
    }

    #[tokio::test]
    async fn persisted_backend_kind_selects_runtime_protocol_without_hostname_guessing() {
        let app = Router::new().route(
            "/api/user/self",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "id": 84,
                        "username": "selected-by-kind",
                        "display_name": "Selected By Kind",
                        "email": "kind@example.com",
                        "group": "default",
                        "quota": 0,
                        "used_quota": 0
                    }
                }))
            }),
        );
        let (origin, server) = spawn(app).await;
        let relay = relay(&origin, BackendKind::NewApi);

        let account = RuntimeBackend::for_relay(&relay).account().await.unwrap();

        assert_eq!(account.login_identifier, "selected-by-kind");
        server.abort();
    }

    #[tokio::test]
    async fn missing_refresh_credential_returns_actionable_session_error() {
        let relay = relay("https://relay.invalid", BackendKind::NewApi);

        let error = RuntimeBackend::for_relay(&relay)
            .refresh_session(None)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("请重新登录"), "{error}");
    }

    #[test]
    fn confirmed_auth_failure_messages_cover_newapi_and_expired_session_prompts() {
        assert!(is_confirmed_auth_failure(&AppError::Config(
            "newapi self 失败: 登录态已失效（HTTP 401），请重新登录中转站账号".into()
        )));
        assert!(is_confirmed_auth_failure(&AppError::Config(
            "登录已过期，请重新登录".into()
        )));
        assert!(!is_confirmed_auth_failure(&AppError::Config(
            "newapi self 请求失败: HTTP 500".into()
        )));
    }
}
