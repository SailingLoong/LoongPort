//! `loongport://connect` 消费端 —— 浏览器接力登录（契约见 `docs/station-connect`）。
//!
//! ## 链路
//!
//! 站点同源握手页（站长挂的 `connect.html`）读本站凭据、用户点击确认后重定向
//! `loongport://connect?origin=…&kind=…&token=…`；OS 深链把 URL 交给本 app。
//! 这里解析参数、**拿凭据真打一次站点 profile**（唯一认证事实），然后走与
//! 登录窗完全相同的落库原语（[`creds::save_credentials`] /
//! [`creds::save_authenticated_relay`]），合并与去重语义免费一致。
//!
//! ## 与登录窗（[`crate::relay::login`]）的关系
//!
//! 同一个终点的两条入口：登录窗在 app 自己的 WebView 里完成登录（能拿到
//! refresh 能力）；浏览器接力复用用户默认浏览器里**已有的会话**（短期、
//! 无 refresh —— 契约有意不收 refresh token，理由见握手页 README）。
//!
//! ## 安全模型
//!
//! - URL 参数全部按不可信处理：认证只认「拿 token 打站点 profile 成功」；
//! - 不接受任何 refresh 凭据（浏览器会话与 app 会话不能共享一次性轮换的凭据）；
//! - `state` 绑定：`begin_browser_login` 发起时登记 nonce+origin（10 分钟 TTL），
//!   带发起凭证的回调必须与发起完全匹配且一次性；无 `state` 的回调视为用户
//!   手动打开握手页，放行（认证事实仍是 profile 验证）；
//! - 坏 token / 坏站点 = 拒收且不落行；用户可见错误走 `deeplink-error`。

use crate::error::AppError;
use crate::relay::backend::BackendKind;
use crate::relay::creds::{
    self, AccountIdentity, AuthenticatedRelay, RelaySite, SessionEnvironment,
};
use crate::store::AppState;

use rusqlite::OptionalExtension;

/// 握手页回传的凭据家族形状（与 `docs/station-connect/README.md` 的 kind 对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectKind {
    /// sub2api：`localStorage.auth_token`（JWT）。
    Sub2Api,
    /// new-api：会话 cookie —— 还要打一次 `GET /api/user/token` 换 access token。
    NewApiSession,
    /// new-api：`user.access_token`（系统访问令牌）。
    NewApiAccessToken,
}

impl ConnectKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "sub2api" => Some(Self::Sub2Api),
            "newapi-session" => Some(Self::NewApiSession),
            "newapi-access-token" => Some(Self::NewApiAccessToken),
            _ => None,
        }
    }

    fn backend(self) -> BackendKind {
        match self {
            Self::Sub2Api => BackendKind::Sub2Api,
            Self::NewApiSession | Self::NewApiAccessToken => BackendKind::NewApi,
        }
    }
}

/// 解析后的握手参数。
#[derive(Debug, Clone)]
pub(crate) struct ConnectHandshake {
    /// 站点面板 origin（`https://host[:port]`，已归一无尾斜杠）。
    pub origin: String,
    pub kind: ConnectKind,
    pub token: String,
    /// 发起端（`begin_browser_login`）生成的 nonce，握手页原样透传；
    /// `None` = 用户手动打开的握手页。
    pub state: Option<String>,
    /// `newapi-session` 换 token 需要的账号 id（`New-Api-User` 头）。
    pub user_id: Option<i64>,
    /// sub2api 的毫秒时间戳字符串（原样透传，落库前归一成秒）。
    pub expires_at: Option<String>,
}

/// 解析 `loongport://connect?…`。参数不合法直接 `Err`，不碰任何状态。
pub(crate) fn parse_connect_url(url: &url::Url) -> Result<ConnectHandshake, AppError> {
    if url.host_str() != Some("connect") {
        return Err(AppError::InvalidInput(format!(
            "connect 深链的 host 不对: {:?}",
            url.host_str()
        )));
    }

    let mut origin = None;
    let mut kind = None;
    let mut token = None;
    let mut state = None;
    let mut user_id = None;
    let mut expires_at = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "origin" => origin = Some(value.into_owned()),
            "kind" => kind = Some(value.into_owned()),
            "token" => token = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "user_id" => user_id = Some(value.into_owned()),
            "expires_at" => expires_at = Some(value.into_owned()),
            // 未知参数忽略：握手页与消费端版本可以不同步。
            _ => {}
        }
    }

    let origin = origin
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("connect 深链缺少 origin".into()))?;
    let parsed = url::Url::parse(&origin)
        .map_err(|e| AppError::InvalidInput(format!("connect 深链的 origin 不合法: {e}")))?;
    // 契约 HTTPS-only；本机 http 只为测试与本地站点放行（与握手页同一条规则）。
    let local = matches!(parsed.host_str(), Some("localhost") | Some("127.0.0.1"));
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return Err(AppError::InvalidInput(
            "connect 深链的 origin 必须是 HTTPS".into(),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(AppError::InvalidInput(
            "connect 深链的 origin 缺少主机名".into(),
        ));
    }
    let origin = parsed.origin().ascii_serialization();

    let kind = kind
        .as_deref()
        .and_then(ConnectKind::parse)
        .ok_or_else(|| AppError::InvalidInput("connect 深链缺少或无法识别 kind".into()))?;

    let token = token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("connect 深链缺少凭据".into()))?;

    let user_id = match user_id
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<i64>())
    {
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return Err(AppError::InvalidInput(
                "connect 深链的 user_id 不是整数".into(),
            ))
        }
        None => None,
    };
    if kind == ConnectKind::NewApiSession && user_id.is_none() {
        return Err(AppError::InvalidInput(
            "newapi-session 换取访问令牌需要 user_id（握手页未从本站读到账号 id）".into(),
        ));
    }

    Ok(ConnectHandshake {
        origin,
        kind,
        token,
        state: state.filter(|s| !s.trim().is_empty()),
        user_id,
        expires_at: expires_at.filter(|s| !s.trim().is_empty()),
    })
}

// ============================================================================
// 发起端：行级「在默认浏览器中登录」按钮 → 握手页带 nonce 打开
// ============================================================================

/// 一次发起中的浏览器接力。`begin_browser_login` 写入、`validate_state_binding`
/// 消费；nonce 把回调绑定到「app 自己发起的那次流程 + 那个站点」。
#[derive(Debug, Clone)]
struct PendingConnect {
    state: String,
    origin: String,
    /// unix 秒。过期的 pending 视同没有（懒清理，不设后台任务）。
    expires_at: i64,
}

const PENDING_CONNECT_TTL_SECS: i64 = 600;

/// 站点握手页的约定路径（契约见 docs/station-connect/README.md）。
const WELL_KNOWN_CONNECT_PATH: &str = "/.well-known/loongport/connect";

static PENDING_CONNECT: std::sync::OnceLock<std::sync::Mutex<Option<PendingConnect>>> =
    std::sync::OnceLock::new();

/// 探测站点握手页部署了没有（只认 2xx；超时短，别让用户干等）。
/// 没部署就别把用户扔到浏览器里看 404 —— 在门口拦下并引导走应用内登录。
async fn probe_connect_page(site_origin: &str) -> Result<(), AppError> {
    let page_url = format!("{site_origin}{WELL_KNOWN_CONNECT_PATH}");
    let status = crate::relay::sub2api::build_client()?
        .get(&page_url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| AppError::Config(format!("探测站点握手页失败（{site_origin}）: {e}")))?
        .status();
    if !status.is_success() {
        return Err(AppError::Config(format!(
            "该站未部署浏览器登录页（HTTP {status}）。可让站长按 docs/station-connect 接入，或改用应用内登录"
        )));
    }
    Ok(())
}

/// 发起浏览器接力登录：探测握手页 → 登记 nonce → 用默认浏览器打开带 state 的页面。
///
/// 用户在浏览器完成登录并点击移交后，深链回来走 [`apply_connect`]。
#[cfg(feature = "gui")]
pub async fn begin_browser_login<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    site_origin: &str,
) -> Result<(), AppError> {
    probe_connect_page(site_origin).await?;

    let state = uuid::Uuid::new_v4().simple().to_string();
    {
        let pending = PENDING_CONNECT.get_or_init(Default::default);
        let mut guard = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(PendingConnect {
            state: state.clone(),
            origin: site_origin.to_string(),
            expires_at: chrono::Utc::now().timestamp() + PENDING_CONNECT_TTL_SECS,
        });
    }

    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(
            format!("{site_origin}{WELL_KNOWN_CONNECT_PATH}?state={state}"),
            None::<String>,
        )
        .map_err(|e| AppError::Config(format!("打开默认浏览器失败: {e}")))?;
    Ok(())
}

/// state 绑定裁决（验证前的准入闸，纯状态机便于测试）：
///
/// - 有进行中的发起（未过期）：state 匹配且 origin 一致 → 放行并**消费**（一次性）；
///   无 state（用户手动开页）→ 放行（用户驱动的流程，pending 留着等真正的绑定回调）；
///   state/origin 对不上 → **拒收**（别的页面往回调里塞凭据）。
/// - 没有进行中的发起：无 state 放行（手动流程）；带 state → 拒收
///   （发起已过期/不存在，stale nonce 不该再被认）。
fn validate_state_binding(handshake: &ConnectHandshake, now: i64) -> Result<(), AppError> {
    let pending_slot = PENDING_CONNECT.get_or_init(Default::default);
    let mut guard = pending_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let pending = match guard.as_ref() {
        Some(p) if p.expires_at > now => Some(p),
        Some(_) => {
            // 懒清理过期发起。
            *guard = None;
            None
        }
        None => None,
    };

    match (pending, handshake.state.as_deref()) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(AppError::InvalidInput(
            "回调带着已失效的发起凭证（发起过期或不存在），请重新发起".into(),
        )),
        (Some(_), None) => Ok(()),
        (Some(p), Some(state)) => {
            if state == p.state && handshake.origin == p.origin {
                // 绑定流程兑现：消费掉，一次发起只认一次回调。
                *guard = None;
                Ok(())
            } else {
                Err(AppError::InvalidInput(
                    "回调与本次发起不匹配（站点或发起凭证不对），已拒收".into(),
                ))
            }
        }
    }
}

/// 落库结果：前端 toast + 档位预配（`ONBOARDING_REGISTER_COMPLETED`）要用的两件事。
pub(crate) struct ConnectOutcome {
    pub relay_id: i64,
    pub site_name: String,
}

/// 验证 + 落库。**唯一认证事实**是「拿凭据打站点 profile 成功」——profile 打不通
/// 的凭据绝不入库。
pub(crate) async fn connect(
    state: &AppState,
    handshake: &ConnectHandshake,
) -> Result<ConnectOutcome, AppError> {
    // 拿到的账号身份（owned：AccountIdentity 是借用视图，落到 save 时再借）。
    struct Verified {
        id: i64,
        label: String,
        login_identifier: String,
        auth_token: String,
        token_expires_at: Option<i64>,
    }

    let verified = match handshake.kind {
        ConnectKind::Sub2Api => {
            let account = crate::relay::sub2api::Client::new(
                &handshake.origin,
                &handshake.token,
                None,
                None,
                None,
            )?
            .account()
            .await
            .map_err(|e| AppError::Config(format!("浏览器接力登录验证失败: {e}")))?;
            Verified {
                id: account.id,
                label: account.display_name(),
                // sub2api 重登预填要邮箱（登录框按邮箱分流，见 login_url 的文档）。
                login_identifier: account.email,
                auth_token: handshake.token.clone(),
                token_expires_at: crate::relay::login::normalize_token_expires_at(
                    handshake.expires_at.as_deref(),
                ),
            }
        }
        ConnectKind::NewApiAccessToken => {
            let account =
                crate::relay::newapi::NewApiClient::new(&handshake.origin, &handshake.token)?
                    .account()
                    .await
                    .map_err(|e| AppError::Config(format!("浏览器接力登录验证失败: {e}")))?;
            let runtime = crate::relay::backend::newapi_runtime_account(&account);
            Verified {
                id: runtime.id,
                label: runtime.label,
                login_identifier: runtime.login_identifier,
                auth_token: handshake.token.clone(),
                token_expires_at: None,
            }
        }
        ConnectKind::NewApiSession => {
            // 会话 cookie 不直接入库（运行时链路全部走 Bearer access token），
            // 借 new-api 自己的「session 换 token」端点换一把 —— 只读语义，
            // 不消耗用户浏览器里的会话。
            let user_id = handshake
                .user_id
                .expect("parse 阶段已保证 newapi-session 带 user_id");
            let refreshed = crate::relay::newapi::exchange_session(
                &handshake.origin,
                &handshake.token,
                user_id,
            )
            .await
            .map_err(|e| AppError::Config(format!("浏览器接力登录验证失败: {e}")))?;
            let runtime = crate::relay::backend::newapi_runtime_account(&refreshed.account);
            Verified {
                id: runtime.id,
                label: runtime.label,
                login_identifier: runtime.login_identifier,
                auth_token: refreshed.access_token,
                token_expires_at: refreshed.access_expires_at,
            }
        }
    };

    // 落库分支与登录窗对齐：已有行只动凭据（save_credentials 不碰
    // site_name/api_base_url —— 不能拿面板 origin 覆盖库里已知的更优事实），
    // 新站才建行（name/api_base 先用 origin 兜底，探测/预配随后会补齐）。
    let identity = AccountIdentity {
        id: verified.id,
        label: &verified.label,
        login_identifier: &verified.login_identifier,
    };
    let site_name_fallback = origin_host(&handshake.origin);
    let relay_id = {
        let conn = crate::database::lock_conn!(state.db.conn);
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM loongport_relay
                 WHERE site_origin = ?1 AND account_id = ?2 LIMIT 1",
                rusqlite::params![handshake.origin, verified.id],
                |row| row.get(0),
            )
            .optional() // map_err below
            .map_err(|e| AppError::Database(format!("查已有中转站行失败: {e}")))?;

        match existing {
            Some(relay_id) => creds::save_credentials(
                &conn,
                relay_id,
                identity,
                &verified.auth_token,
                // 接力产物没有 refresh（有意，见模块文档）；显式传 None，
                // 覆盖旧行上可能存在的旧 refresh 凭据 —— 它属于旧的会话族。
                None,
                verified.token_expires_at,
                SessionEnvironment::default(),
            )?,
            None => creds::save_authenticated_relay(
                &conn,
                AuthenticatedRelay {
                    site: RelaySite {
                        site_origin: &handshake.origin,
                        site_name: &site_name_fallback,
                        api_base_url: &handshake.origin,
                        backend_kind: handshake.kind.backend(),
                    },
                    account: identity,
                    auth_token: &verified.auth_token,
                    refresh_token: None,
                    token_expires_at: verified.token_expires_at,
                    session: SessionEnvironment::default(),
                },
            )?,
        }
    };

    // toast 用库里那行最终的名字（已有行保留原站点名）。
    let site_name: String = {
        let conn = crate::database::lock_conn!(state.db.conn);
        conn.query_row(
            "SELECT site_name FROM loongport_relay WHERE id = ?1",
            rusqlite::params![relay_id],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Database(format!("读中转站站名失败: {e}")))?
    };

    Ok(ConnectOutcome {
        relay_id,
        site_name,
    })
}

/// origin → 展示名兜底（`https://panel.example` → `panel.example`）。
fn origin_host(origin: &str) -> String {
    origin
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(feature = "gui")]
/// 深链入口：解析 → 验证落库 → 成功发 [`crate::events::ONBOARDING_REGISTER_COMPLETED`]
/// （toast + 档位预配 + 列表刷新，与注册窗共用一套收尾）；失败发 `deeplink-error`。
pub async fn apply_connect<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>, url_str: &str) {
    use tauri::{Emitter, Manager};

    let parsed = url::Url::parse(url_str)
        .map_err(|e| AppError::InvalidInput(format!("connect 深链 URL 不合法: {e}")))
        .and_then(|url| parse_connect_url(&url));
    let handshake = match parsed {
        Ok(handshake) => handshake,
        Err(error) => {
            emit_deeplink_error(app_handle, url_str, &error);
            return;
        }
    };

    log::info!(
        "[browser-connect] 收到接力登录: origin={} kind={:?}",
        handshake.origin,
        handshake.kind
    );

    // 准入闸：state 绑定（有发起在途时严格匹配 origin+nonce；见 validate_state_binding）。
    if let Err(error) = validate_state_binding(&handshake, chrono::Utc::now().timestamp()) {
        log::warn!("[browser-connect] {error}");
        emit_deeplink_error(app_handle, url_str, &error);
        return;
    }

    let state = app_handle.state::<AppState>();
    match connect(&state, &handshake).await {
        Ok(outcome) => {
            log::info!(
                "[browser-connect] 接力登录完成: relay #{}（{}）",
                outcome.relay_id,
                outcome.site_name
            );
            if let Err(error) = app_handle.emit(
                crate::events::ONBOARDING_REGISTER_COMPLETED,
                crate::events::RegisterCompletedPayload {
                    relay_id: outcome.relay_id,
                    site_name: outcome.site_name,
                },
            ) {
                log::warn!("[browser-connect] 发完成事件失败: {error}");
            }
        }
        Err(error) => {
            log::warn!("[browser-connect] 接力登录被拒: {error}");
            emit_deeplink_error(app_handle, url_str, &error);
        }
    }
}

#[cfg(feature = "gui")]
fn emit_deeplink_error<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    url: &str,
    error: &AppError,
) {
    use tauri::Emitter;

    if let Err(emit_error) = app_handle.emit(
        "deeplink-error",
        serde_json::json!({ "url": url, "error": error.to_string() }),
    ) {
        log::error!("[browser-connect] 发 deeplink-error 失败: {emit_error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_server(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (origin, task)
    }

    fn connect_url(origin: &str, kind: &str, token: &str, extra: &str) -> String {
        format!("loongport://connect?origin={origin}&kind={kind}&token={token}{extra}")
    }

    fn parse(url: &str) -> Result<ConnectHandshake, AppError> {
        parse_connect_url(&url::Url::parse(url).expect("测试 URL 必须合法"))
    }

    fn state() -> AppState {
        AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().expect("内存库"),
        ))
    }

    fn sub2api_profile_router(expected_token: &str, user_id: i64) -> axum::routing::MethodRouter {
        use axum::http::HeaderMap;
        use axum::routing::get;
        use axum::Json;
        let expected = expected_token.to_string();
        get(move |headers: HeaderMap| async move {
            let bearer_ok = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == format!("Bearer {expected}"));
            if bearer_ok {
                Ok(Json(serde_json::json!({
                    "code": 0, "message": "success",
                    "data": { "id": user_id, "username": "测试用户", "email": "u@example.com",
                              "balance": 1.0, "frozen_balance": 0.0 }
                })))
            } else {
                Err(axum::http::StatusCode::UNAUTHORIZED)
            }
        })
    }

    fn newapi_self_router(expected_token: &str, user_id: i64) -> axum::Router {
        use axum::http::HeaderMap;
        use axum::routing::get;
        use axum::Json;
        let expected = expected_token.to_string();
        axum::Router::new().route(
            "/api/user/self",
            get(move |headers: HeaderMap| async move {
                let ok = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v == format!("Bearer {expected}"));
                if ok {
                    Ok(Json(serde_json::json!({
                        "success": true, "message": "",
                        "data": { "id": user_id, "username": "newapi-user",
                                  "display_name": "展示名", "email": "n@example.com",
                                  "group": "default", "quota": 1, "used_quota": 0 }
                    })))
                } else {
                    Err(axum::http::StatusCode::UNAUTHORIZED)
                }
            }),
        )
    }

    fn relay_row(state: &AppState, relay_id: i64) -> (String, String, String, Option<i64>) {
        let conn = state.db.conn.lock().expect("锁内存库");
        conn.query_row(
            "SELECT site_origin, api_base_url, backend_kind, account_id
             FROM loongport_relay WHERE id = ?1",
            rusqlite::params![relay_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    }

    fn relay_count(state: &AppState) -> i64 {
        let conn = state.db.conn.lock().expect("锁内存库");
        conn.query_row("SELECT count(*) FROM loongport_relay", [], |row| row.get(0))
            .unwrap()
    }

    // ==================== 解析 ====================

    #[test]
    fn parse_accepts_all_kinds_and_normalizes_origin() {
        let hs = parse(&connect_url(
            "https%3A%2F%2Fpanel.example%2Flogin",
            "sub2api",
            "jwt-abc",
            "&expires_at=1780000000000",
        ))
        .expect("合法握手");
        assert_eq!(
            hs.origin, "https://panel.example",
            "origin 归一到根并剥路径"
        );
        assert_eq!(hs.kind, ConnectKind::Sub2Api);
        assert_eq!(hs.token, "jwt-abc");
        assert_eq!(hs.expires_at.as_deref(), Some("1780000000000"));

        let hs = parse(&connect_url(
            "http%3A%2F%2F127.0.0.1%3A8901",
            "newapi-access-token",
            "sys-token",
            "",
        ))
        .expect("本机 http 放行（测试与本地站点）");
        assert_eq!(hs.origin, "http://127.0.0.1:8901");
    }

    #[test]
    fn parse_rejects_bad_handshakes() {
        assert!(parse(&connect_url(
            "https%3A%2F%2Fpanel.example",
            "sub2api",
            "",
            ""
        ))
        .is_err());
        assert!(parse("loongport://connect?kind=sub2api&token=t").is_err());
        assert!(parse(&connect_url(
            "http%3A%2F%2Fpanel.example",
            "sub2api",
            "t",
            ""
        ))
        .is_err());
        assert!(parse(&connect_url(
            "https%3A%2F%2Fa.example",
            "unknown-kind",
            "t",
            ""
        ))
        .is_err());
        // newapi-session 缺 user_id：换 token 需要 New-Api-User 头
        assert!(parse(&connect_url(
            "https%3A%2F%2Fa.example",
            "newapi-session",
            "sess",
            ""
        ))
        .is_err());
        assert!(parse(&connect_url(
            "https%3A%2F%2Fa.example",
            "newapi-session",
            "sess",
            "&user_id=42"
        ))
        .is_ok());
        // host 不是 connect 的不是本模块的 URL
        assert!(parse("loongport://import/v1?x=1").is_err());
    }

    // ==================== 验证 + 落库 ====================

    /// sub2api 正路：验证成功建行（backend/账号/过期毫秒归一秒），二次接力合并不建行。
    #[tokio::test]
    async fn sub2api_connect_creates_then_merges() {
        let (origin, _server) = spawn_server(
            axum::Router::new().route("/api/v1/user/profile", sub2api_profile_router("jwt-abc", 7)),
        )
        .await;
        let encoded_origin = urlencoding_lite(&origin);
        let state = state();

        let first = connect(
            &state,
            &parse(&connect_url(
                &encoded_origin,
                "sub2api",
                "jwt-abc",
                "&expires_at=1780000000000",
            ))
            .unwrap(),
        )
        .await
        .expect("首连成功");
        assert_eq!(first.site_name, origin_host(&origin), "新站名兜底用主机名");
        let (site_origin, api_base, backend, account_id) = relay_row(&state, first.relay_id);
        assert_eq!(site_origin, origin);
        assert_eq!(api_base, origin);
        assert_eq!(backend, "sub2api");
        assert_eq!(account_id, Some(7));
        let expires: Option<i64> = {
            let conn = state.db.conn.lock().expect("锁内存库");
            conn.query_row(
                "SELECT token_expires_at FROM loongport_relay WHERE id = ?1",
                rusqlite::params![first.relay_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(expires, Some(1_780_000_000), "毫秒归一成秒");

        // 二次接力（同站同账号）：合并进同一行，不建新行
        let second = connect(
            &state,
            &parse(&connect_url(&encoded_origin, "sub2api", "jwt-abc", "")).unwrap(),
        )
        .await
        .expect("二次接力成功");
        assert_eq!(second.relay_id, first.relay_id);
        assert_eq!(relay_count(&state), 1);
    }

    /// new-api access_token 正路。
    #[tokio::test]
    async fn newapi_access_token_connect_creates_row() {
        let (origin, _server) = spawn_server(newapi_self_router("sys-token", 42)).await;
        let state = state();

        let outcome = connect(
            &state,
            &parse(&connect_url(
                &urlencoding_lite(&origin),
                "newapi-access-token",
                "sys-token",
                "",
            ))
            .unwrap(),
        )
        .await
        .expect("access token 接力成功");

        let (site_origin, _, backend, account_id) = relay_row(&state, outcome.relay_id);
        assert_eq!(site_origin, origin);
        assert_eq!(backend, "newapi");
        assert_eq!(account_id, Some(42));
    }

    /// new-api 会话 cookie 正路：先换 access token（/api/user/token）再拉 self，
    /// 入库的是换出来的 Bearer 令牌而不是会话 cookie。
    #[tokio::test]
    async fn newapi_session_connect_exchanges_for_access_token() {
        use axum::routing::get;
        let router = axum::Router::new()
            .route(
                "/api/user/token",
                get(|headers: axum::http::HeaderMap| async move {
                    let cookie_ok = headers
                        .get(axum::http::header::COOKIE)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|c| c.contains("session=sess-cookie"));
                    let user_header_ok = headers
                        .get("new-api-user")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v == "42");
                    if cookie_ok && user_header_ok {
                        Ok(axum::Json(serde_json::json!({
                            "success": true, "message": "", "data": "exchanged-bearer"
                        })))
                    } else {
                        Err(axum::http::StatusCode::UNAUTHORIZED)
                    }
                }),
            )
            .merge(newapi_self_router("exchanged-bearer", 42));
        let (origin, _server) = spawn_server(router).await;
        let state = state();

        let outcome = connect(
            &state,
            &parse(&connect_url(
                &urlencoding_lite(&origin),
                "newapi-session",
                "sess-cookie",
                "&user_id=42",
            ))
            .unwrap(),
        )
        .await
        .expect("会话接力成功");

        let stored_token: String = {
            let conn = state.db.conn.lock().expect("锁内存库");
            conn.query_row(
                "SELECT auth_token FROM loongport_relay WHERE id = ?1",
                rusqlite::params![outcome.relay_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            stored_token, "exchanged-bearer",
            "入库的是换出来的 Bearer 令牌，不是会话 cookie"
        );
    }

    /// ⭐ 坏凭据拒收：profile 打不通（401）绝不能落行 —— URL 参数不可信，
    /// profile 成功是唯一认证事实。
    #[tokio::test]
    async fn invalid_token_is_rejected_without_persisting() {
        let (origin, _server) = spawn_server(axum::Router::new().route(
            "/api/v1/user/profile",
            sub2api_profile_router("real-token", 7),
        ))
        .await;
        let state = state();

        let result = connect(
            &state,
            &parse(&connect_url(
                &urlencoding_lite(&origin),
                "sub2api",
                "forged-token",
                "",
            ))
            .unwrap(),
        )
        .await;
        assert!(result.is_err(), "假 token 必须被站点 profile 拒收");
        assert_eq!(relay_count(&state), 0, "拒收即不落行");
    }

    /// 测试 URL 里的 origin 需要 percent-encode（host 带端口时 `:` 与参数分隔冲突）。
    fn urlencoding_lite(s: &str) -> String {
        s.replace(':', "%3A").replace('/', "%2F")
    }

    // ==================== 发起端与 state 绑定 ====================

    fn reset_pending(state: Option<(&str, &str, i64)>) {
        let slot = PENDING_CONNECT.get_or_init(Default::default);
        let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = state.map(|(state, origin, expires_at)| PendingConnect {
            state: state.to_string(),
            origin: origin.to_string(),
            expires_at,
        });
    }

    fn handshake_with(origin: &str, state: Option<&str>) -> ConnectHandshake {
        ConnectHandshake {
            origin: origin.to_string(),
            kind: ConnectKind::Sub2Api,
            token: "t".into(),
            state: state.map(str::to_string),
            user_id: None,
            expires_at: None,
        }
    }

    /// ⭐ state 绑定状态机：共享全局 pending，必须串行跑。
    #[test]
    #[serial_test::serial]
    fn state_binding_full_lifecycle() {
        let now = 10_000_i64;

        // 无发起：无 state 放行（手动开页），带 state 拒收（stale nonce 不认）
        reset_pending(None);
        assert!(validate_state_binding(&handshake_with("https://a.example", None), now).is_ok());
        assert!(
            validate_state_binding(&handshake_with("https://a.example", Some("stale")), now)
                .is_err()
        );

        // 有发起：匹配的 state+origin 放行且**一次性消费**
        reset_pending(Some(("nonce-1", "https://a.example", now + 60)));
        let matched = handshake_with("https://a.example", Some("nonce-1"));
        assert!(validate_state_binding(&matched, now).is_ok());
        assert!(
            validate_state_binding(&matched, now).is_err(),
            "绑定兑现后 pending 已消费，重放同一个回调必须被拒"
        );

        // 有发起：错 state / 错 origin 拒收，但 pending 不被消耗（真正的回调仍可兑现）
        reset_pending(Some(("nonce-2", "https://a.example", now + 60)));
        assert!(
            validate_state_binding(&handshake_with("https://a.example", Some("evil")), now)
                .is_err()
        );
        assert!(
            validate_state_binding(&handshake_with("https://b.example", Some("nonce-2")), now)
                .is_err()
        );
        assert!(
            validate_state_binding(&handshake_with("https://a.example", Some("nonce-2")), now)
                .is_ok()
        );

        // 有发起：无 state（手动开页）放行，pending 留给真正的绑定回调
        reset_pending(Some(("nonce-3", "https://a.example", now + 60)));
        assert!(validate_state_binding(&handshake_with("https://b.example", None), now).is_ok());
        assert!(
            validate_state_binding(&handshake_with("https://a.example", Some("nonce-3")), now)
                .is_ok()
        );

        // 过期发起：视同没有（懒清理）—— 无 state 放行、带 state 拒收
        reset_pending(Some(("nonce-4", "https://a.example", now - 1)));
        assert!(validate_state_binding(&handshake_with("https://a.example", None), now).is_ok());
        assert!(
            validate_state_binding(&handshake_with("https://a.example", Some("nonce-4")), now)
                .is_err()
        );

        reset_pending(None);
    }

    /// 探测闸：握手页在（2xx）放行到「打开浏览器」，不在（404）给出可行动的错误。
    #[tokio::test]
    async fn probe_accepts_deployed_page_and_rejects_missing() {
        use axum::routing::get;
        let (origin, _server) = spawn_server(
            axum::Router::new()
                .route(
                    "/.well-known/loongport/connect",
                    get(|| async { "<html>connect</html>" }),
                )
                .route(
                    "/.well-known/missing",
                    get(|| async { axum::http::StatusCode::NOT_FOUND }),
                ),
        )
        .await;

        assert!(probe_connect_page(&origin).await.is_ok());
        let err = probe_connect_page(&format!("{origin}/well-known"))
            .await
            .err();
        // 路径拼错 → 404 → 错误信息要能指导行动（提站点未部署）
        assert!(
            err.is_some_and(|e| e.to_string().contains("未部署")),
            "未部署站点的错误要指明原因"
        );
    }
}
