//! 中转站协议适配器的共享契约。
//!
//! 协议模块拥有 endpoint、wire DTO 和 detector；discovery 只遍历这里的窄描述符，
//! 不携带任何协议专属响应类型。

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::relay::{creds, login, newapi, sub2api};

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
    /// 指纹实际被观察到的 origin。原生探针跟随重定向后可能落在与请求不同的
    /// origin（典型：裸域 301 到 `www.`）；浏览器路径的回传总在锚点 origin 上，
    /// 故为 `None`。导入窗据此把入口、脚本守卫与落库行锚到页面真正停留的 origin。
    pub final_origin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeCandidate {
    pub id: &'static str,
    pub path: &'static str,
    /// 可选的页面登录令牌键。仅在目标 origin 内给该候选请求补 Bearer 头；
    /// 令牌不回传 Rust、不写日志，也不与其它协议候选共享。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token_storage_key: Option<&'static str>,
    /// JSON paths required by this adapter's strict detector when a public response is too large
    /// to return verbatim through the bounded WebView callback.
    pub detector_json_paths: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub struct ProbeAdapter {
    pub candidate: ProbeCandidate,
    pub detect: fn(&str) -> Option<DetectedSite>,
}

pub fn browser_login_url(
    site_origin: &str,
    backend_kind: BackendKind,
    login_identifier: &str,
) -> String {
    match backend_kind {
        BackendKind::Sub2Api => login::login_url(site_origin, login_identifier),
        BackendKind::NewApi => newapi::login_url(site_origin, login_identifier),
    }
}

pub fn browser_login_script(
    site_origin: &str,
    backend_kind: BackendKind,
    login_identifier: &str,
    aff_code: Option<&str>,
    promo_code: Option<&str>,
) -> String {
    match backend_kind {
        BackendKind::Sub2Api => {
            login::login_script(site_origin, login_identifier, aff_code, promo_code)
        }
        BackendKind::NewApi => newapi::login_script(),
    }
}

/// 把一个 `reqwest` 发送错误描述成**能定位问题**的一行。api / newapi 两条协议侧共用。
///
/// ## 为什么不能直接 `{e}`
///
/// `reqwest::Error` 的 `Display` 只打印最外层，形如
/// `error sending request for url (https://…)` —— **超时、DNS 解析失败、连接被拒、
/// TLS 握手失败、代理不可达打印出来完全一样**，真实原因在 `std::error::Error::source()`
/// 链里（hyper → 系统错误）。
///
/// 2026-08-03 的实测代价：用户报「获取分组列表失败: error sending request for url
/// (https://bestapi.example/api/v1/groups/available)」，日志里就这一句。为判断是哪一类，
/// 只能专门写一个最小复现程序传到那台 Windows 上，逐层验证 DNS / TCP / TLS / 5 种 client
/// 变体 —— 而那本该是日志里现成的一行。
///
/// 所以这里做两件事：**给出失败类别**（`is_timeout` / `is_connect` 这些谓词，比原始
/// 措辞更适合展示给用户），以及**展开整条 source 链**（给维护者定位用）。
pub(crate) fn describe_send_error(e: &reqwest::Error) -> String {
    // 类别前缀：用户看得懂的话术。判定顺序按「越具体越先」——
    // 超时优先于连接：连接阶段超时时两个谓词可能同时为真，而「超时」对用户更有指导性
    // （等一下重试），「连不上」会让人以为是地址错了。
    let kind = if e.is_timeout() {
        "请求超时"
    } else if e.is_connect() {
        "连不上服务器"
    } else if e.is_request() {
        "请求发送失败"
    } else {
        "网络错误"
    };

    let mut out = format!("{kind}（{e}）");
    // 整条链都带上：真实原因常在第 2-3 层（hyper 之下的系统错误）。
    // 首层往往就是判据本身（超时是 `operation timed out`、连接被拒是系统 errno）。
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(&format!(" cause: {s}"));
        src = s.source();
    }
    out
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

/// 401 文案的**措辞标记**：api / newapi 两条协议侧格式化错误时必须用这些常量拼，
/// 这边的谓词按它们分拣。各写一遍字面量迟早分叉，分叉的后果见
/// [`is_token_expiry_failure`] 的文档。
pub const AUTH_DEAD_MARKER: &str = "登录态已失效";
pub const AUTH_EXPIRED_MARKER: &str = "登录已过期";
pub const RELOGIN_MARKER: &str = "请重新登录";

pub fn is_confirmed_auth_failure(error: &AppError) -> bool {
    match error {
        AppError::Config(message)
        | AppError::InvalidInput(message)
        | AppError::Message(message) => {
            message.contains(AUTH_DEAD_MARKER) || message.contains(RELOGIN_MARKER)
        }
        _ => false,
    }
}

/// 「token 过期、但 refresh 可能救得回来」的那一类 401，与 [`is_confirmed_auth_failure`]
/// 里「账号已死」那类（被禁用 / 会话被撤销 / 用户不存在）相对 —— 后者续期也无济于事。
///
/// 这是「过期了靠 401 发现」那半句承诺的兑现处：`token_expires_at = NULL` 的乐观降级行
/// （登录快照没带回过期时间的站点）永远不会触发主动续期，唯一的自救机会就是撞上
/// 过期 401 之后先续期一次再重试（见 `commands::relay::relay_read_with_refresh_retry`）。
///
/// NewAPI 的 401 文案落在「已失效」那类、**有意**不在此列：它的 refresh cookie 有
/// 30 秒 reuse 判定，拿可能已被消费的 cookie 盲目重试会吊销整个会话族。
pub fn is_token_expiry_failure(error: &AppError) -> bool {
    match error {
        AppError::Config(message)
        | AppError::InvalidInput(message)
        | AppError::Message(message) => message.contains(AUTH_EXPIRED_MARKER),
        _ => false,
    }
}

pub enum RuntimeBackend<'a> {
    Sub2Api { relay: &'a creds::RelayAccount },
    NewApi { relay: &'a creds::RelayAccount },
}

impl<'a> RuntimeBackend<'a> {
    pub fn for_relay(relay: &'a creds::RelayAccount) -> Self {
        match relay.backend_kind {
            BackendKind::Sub2Api => Self::Sub2Api { relay },
            BackendKind::NewApi => Self::NewApi { relay },
        }
    }

    pub async fn account(&self) -> Result<RuntimeAccount, AppError> {
        match self {
            Self::Sub2Api { relay } => {
                let account = sub2api::Client::new(
                    &relay.site_origin,
                    &relay.auth_token,
                    relay.account_id,
                    relay.user_agent.as_deref(),
                    relay.cf_clearance.as_deref(),
                )?
                .account()
                .await?;
                Ok(RuntimeAccount {
                    id: account.id,
                    label: account.display_name(),
                    login_identifier: account.email,
                })
            }
            Self::NewApi { relay } => {
                let account = newapi::NewApiClient::with_optional_account_id(
                    &relay.site_origin,
                    &relay.auth_token,
                    relay.account_id,
                )?
                .account()
                .await?;
                Ok(newapi_runtime_account(&account))
            }
        }
    }

    pub async fn balance(&self) -> Result<RuntimeBalance, AppError> {
        match self {
            Self::Sub2Api { relay } => {
                let balance = sub2api::Client::new(
                    &relay.site_origin,
                    &relay.auth_token,
                    relay.account_id,
                    relay.user_agent.as_deref(),
                    relay.cf_clearance.as_deref(),
                )?
                .balance()
                .await?;
                Ok(RuntimeBalance {
                    balance: balance.balance,
                    frozen_balance: balance.frozen_balance,
                })
            }
            Self::NewApi { relay } => {
                let account = newapi::NewApiClient::with_optional_account_id(
                    &relay.site_origin,
                    &relay.auth_token,
                    relay.account_id,
                )?
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
                let refreshed =
                    sub2api::refresh_token(&relay.site_origin, refresh_credential).await?;
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
                    token_expires_at: refreshed.access_expires_at,
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

    #[test]
    fn browser_login_dispatch_keeps_protocol_details_out_of_commands() {
        assert_eq!(
            browser_login_url("https://api.example.com", BackendKind::Sub2Api, ""),
            "https://api.example.com/register"
        );
        assert_eq!(
            browser_login_url(
                "https://api.example.com",
                BackendKind::NewApi,
                "newapi-login"
            ),
            "https://api.example.com/login"
        );
        assert!(!browser_login_script(
            "https://api.example.com",
            BackendKind::Sub2Api,
            "",
            None,
            None
        )
        .is_empty());
        assert!(!browser_login_script(
            "https://api.example.com",
            BackendKind::NewApi,
            "",
            None,
            None
        )
        .is_empty());

        // commands::relay 已按领域拆成目录（2026-09-07），这里显式枚举全部模块文件 ——
        // 加新模块时记得补一行。
        let command_source = [
            include_str!("../commands/relay/mod.rs"),
            include_str!("../commands/relay/balance.rs"),
            include_str!("../commands/relay/directory.rs"),
            include_str!("../commands/relay/imagegen.rs"),
            include_str!("../commands/relay/login.rs"),
            include_str!("../commands/relay/official.rs"),
            include_str!("../commands/relay/provision.rs"),
            include_str!("../commands/relay/rows.rs"),
            include_str!("../commands/relay/session.rs"),
            include_str!("../commands/relay/site_config.rs"),
            include_str!("../commands/relay/switch.rs"),
            include_str!("../commands/relay/windows.rs"),
        ]
        .concat();
        for protocol_detail in [
            "new_api_refresh",
            "loongport-newapi-session",
            "New-Api-User",
            "/api/user/auth/refresh",
            "/api/user/token",
            "fn backend_login_url(",
            "fn import_login_script(",
        ] {
            assert!(
                !command_source.contains(protocol_detail),
                "commands::relay must not own protocol detail {protocol_detail:?}"
            );
        }
    }

    /// **传输错误必须带上 `source()` 链**。
    ///
    /// 2026-08-03 实测代价：用户报「获取分组列表失败: error sending request for url
    /// (https://panel.example/api/v1/groups/available)」，而那串正是 `{e}` 对
    /// `reqwest::Error` 的全部输出 —— 超时 / DNS / 连接被拒 / TLS 失败**打印出来一模一样**，
    /// 真实原因在 `source()` 链里被丢掉了。结果：为了知道是哪一种，只能专门编一个最小
    /// 复现程序传到那台机器上跑（DNS、TCP、TLS、5 个 client 变体逐层验证），
    /// 而这本该是日志里的一行。
    ///
    /// 这条钉住「链被展开了」，不钉具体措辞（那是 reqwest 的措辞，会随版本变）。
    ///
    /// ## 两处刻意的写法
    ///
    /// **1. 用本机 listener 制造失败，不打任何外部地址。** 起初用的是保留地址
    /// `192.0.2.1`（RFC 5737）+ 1ms 超时，但那不由本进程说了算：CI 的网络命名空间可能
    /// 立即回 network-unreachable（那是 connect 而非 timeout）、透明代理也可能把它接走
    /// 变成一个 HTTP 响应 ⇒ 拿到 `Ok` 而不是错误。现在连一个**接受连接但永不回应**的
    /// 本机 listener，超时由我们自己的 timeout 决定，与外网和 CI 网络配置无关。
    ///
    /// **2. 断言比对首层 source 的原文**，而不是「长度变长了」或「包含某个词」——
    /// 那两种都能被类别前缀单独满足，把 source 遍历整段删掉测试照样过（codex review
    /// 抓到的正是这一点）。
    #[tokio::test]
    async fn transport_errors_carry_their_source_chain() {
        // 接受连接后什么都不做（连 listener 都不 drop）⇒ 客户端等响应等到超时。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            // 握住连接不放：一 drop 就变成「连接被对端关闭」，那是另一类错误。
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        // `.no_proxy()` 不可省，理由见下一条测试。
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .no_proxy()
            .build()
            .expect("build client");
        let err = client
            .get(format!("http://{addr}/whatever"))
            .send()
            .await
            .expect_err("对端永不回应，必须超时");

        let bare = format!("{err}");
        let described = describe_send_error(&err);

        // 前提一：`{e}` 真的什么都没说 —— 这正是 bug 的形状。
        assert!(
            !bare.contains("cause"),
            "`{{e}}` 不该带 cause，否则这条测试的前提不成立: {bare}"
        );
        // 前提二：这个错误确实有 source 可展开（没有的话下面的断言就是空转）。
        let first = std::error::Error::source(&err).expect("传输错误必须有 source 可展开");

        // 本体：首层 source 的原文必须出现在描述里。删掉 source 遍历这条就会红。
        assert!(
            described.contains(&format!("cause: {first}")),
            "描述必须带上首层 source 的原文\n  source: {first}\n  desc: {described}"
        );
    }

    /// 分类前缀不能张冠李戴：连接失败不该被说成超时。
    #[tokio::test]
    async fn describe_send_error_labels_connect_failures_as_connect() {
        // 先绑一个端口再立即释放 ⇒ 拿到一个**确定没人监听**的地址。
        // 不用写死的 `127.0.0.1:1`：没有哪条规矩保证它在所有 CI 上都空着。
        let addr = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind probe");
            probe.local_addr().expect("local addr")
            // probe 在这里 drop，端口回到无人监听状态。
        };

        // `.no_proxy()` 不可省：维护者机器上开着 Clash，系统代理会把这个请求接走并回
        // **503**（`proxy-connection: close`）⇒ 拿到的是 `Ok(response)` 而不是传输错误，
        // 测试会以「这个端口竟然有人监听」的形式失败。这里要的是「连接失败」这个事件本身，
        // 不该受运行环境有没有代理影响。
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let err = client
            .get(format!("http://{addr}/whatever"))
            .send()
            .await
            .expect_err("刚释放的端口不该有人监听");

        // 前提：这确实是一个连接类失败、且不是超时。否则下面在验别的东西。
        assert!(err.is_connect(), "前提不成立，这不是连接类错误: {err:?}");
        assert!(!err.is_timeout(), "前提不成立，这是超时错误: {err:?}");

        // 本体：分类前缀必须如实说「连不上」，不能标成超时或含糊的兜底措辞。
        let described = describe_send_error(&err);
        assert!(
            described.starts_with("连不上服务器"),
            "连接类失败的分类前缀必须是「连不上服务器」: {described}"
        );
    }

    use super::*;
    use crate::relay::creds::RelayAccount;

    fn relay(origin: &str, backend_kind: BackendKind) -> RelayAccount {
        RelayAccount {
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
            user_agent: None,
            cf_clearance: None,
            pricing_synced_at: None,
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
                            "system_name": "RelayAccount",
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

    #[test]
    fn token_expiry_failure_is_disjoint_from_dead_account_wording() {
        let expired = AppError::Config(format!(
            "获取余额失败: {AUTH_EXPIRED_MARKER}（TOKEN_EXPIRED），{RELOGIN_MARKER}"
        ));
        let dead = AppError::Config(format!(
            "获取余额失败: {AUTH_DEAD_MARKER}（USER_INACTIVE），{RELOGIN_MARKER}中转站账号"
        ));

        assert!(is_token_expiry_failure(&expired), "{expired}");
        // 账号已死（被禁用 / 会话被撤销）那类 401 续期救不回来，不该触发续期重试。
        assert!(!is_token_expiry_failure(&dead), "{dead}");
        assert!(!is_token_expiry_failure(&AppError::Config(
            "获取余额失败: HTTP 502 Bad Gateway".into()
        )));
        assert!(!is_token_expiry_failure(&AppError::Database(
            "数据库被锁".into()
        )));

        // 措辞闸：两类标记互不为子串 —— 「登录已过期」一旦被写进「登录态已失效」
        // 的文案，过期 401 会被误判成账号已死、永远不触发续期重试。
        assert!(!AUTH_DEAD_MARKER.contains(AUTH_EXPIRED_MARKER));
        assert!(!AUTH_EXPIRED_MARKER.contains(AUTH_DEAD_MARKER));
    }
}
