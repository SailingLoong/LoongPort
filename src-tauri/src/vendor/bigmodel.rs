//! 智谱 BigModel（bigmodel.cn）官网直连账号。
//!
//! 与 [`super::deepseek`] 平级并列，公开面按同构形状组织：登录窗脚本 +
//! 凭据回传、REST key 管理、余额、`config_for` 预设。差异点都在本模块内部消化：
//!
//! - **会话凭据是三件套**：JWT（cookie `bigmodel_token_production`，非 HttpOnly）+
//!   组织 id + 项目 id（localStorage）。`auth_token` 列里存三者的 JSON（[`Session`]），
//!   列语义不变（「调用厂商 API 所需的全部凭据材料」），表结构零迁移。
//! - **鉴权头**：JWT 裸放 `authorization`（无 Bearer 前缀），另需
//!   `bigmodel-organization` / `bigmodel-project` 两个头。
//! - **key 列表直接返回明文**（实测 2026-08-15）⇒ 认领路径比 DeepSeek 还直接：
//!   列表 → 按名字认领，没有再创建。
//! - **登录方式**：微信扫码 / 手机验证码 / 账号密码，成功统一落到 `/console/overview`。
//!   凭据不劫持响应，而是**轮询** cookie + localStorage 三件套齐了就回传 ——
//!   三种登录方式的落点完全一致，轮询对三条路都成立。
//!
//! ## 实测依据
//!
//! 端点与存储结构见 design 私仓 `vendor-integration/bigmodel-opencode-逆向实录-20260815.md`。

use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};

use crate::error::AppError;
use crate::provider::UsageResult;
use crate::vendor::{VendorAccount, VendorError, VendorKey};

/// 凭据回传用的自定义 scheme。
///
/// ⚠️ **与 deepseek 那个不同名**：同 scheme 会让两个登录窗互相认走对方的回传
/// （`deepseek::parse_creds_navigation` 的文档警告过这个坑）。
const CREDS_SCHEME: &str = "loongport-bigmodel-creds";

/// 稳定标识（`Vendor::vendor_id` 与单 plan 段的唯一源）。⚠️ 改它是迁移不是重构。
pub const VENDOR_ID: &str = "bigmodel";

/// 登录窗 label。与 deepseek 分开：两个厂商的登录窗可以同时开。
pub const LOGIN_WINDOW_LABEL: &str = "loongport-bigmodel-login";

pub const SITE_ORIGIN: &str = "https://www.bigmodel.cn";

/// 功能性登录页（邀请链接由远端配置覆盖，见 `commands::vendor::do_login`）。
pub const LOGIN_URL: &str = "https://www.bigmodel.cn/login?redirect=%2F";

/// 「管理 API Key」页面，给用户跳转用。
pub const API_KEYS_URL: &str = "https://bigmodel.cn/apikey/platform";

/// OpenAI 兼容 API 根（chat/completions 在 `{根}/chat/completions`）。
pub const API_ORIGIN: &str = "https://open.bigmodel.cn/api/paas/v4";

/// Anthropic 兼容层（GLM Coding Plan），挂子路径 —— 与 DeepSeek 的 `/anthropic` 同构。
pub const ANTHROPIC_ORIGIN: &str = "https://open.bigmodel.cn/api/anthropic";

/// 旗舰档。`config_for` 的默认模型；远端 `tier_configs` 可覆盖。
const FLAGSHIP: &str = "glm-5.2";
/// 便宜档。
const TURBO: &str = "glm-5-turbo";

// ─────────────────────── 会话凭据（auth_token 列的形态） ───────────────────────

/// 调用 key/余额 API 所需的三件套，序列化成 JSON 存进 `auth_token` 列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// cookie `bigmodel_token_production` 的裸 JWT。
    pub token: String,
    /// localStorage `Bigmodel-Organization`。
    pub org: String,
    /// localStorage `Bigmodel-Project`。
    pub project: String,
}

/// 从 `auth_token` 列解析会话。空串 = 从没登录过（与 deepseek 的语义一致）。
pub fn parse_session(auth_token: &str) -> Result<Session, AppError> {
    if auth_token.trim().is_empty() {
        return Err(AppError::Config("这个官网账号没有登录态".into()));
    }
    serde_json::from_str(auth_token)
        .map_err(|e| AppError::Config(format!("智谱登录态格式不对: {e}")))
}

// ─────────────────────────── 信封与 HTTP ───────────────────────────

/// bigmodel 的单层信封：`{"code":200,"msg":"…","data":…,"success":true}`。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    msg: String,
    data: Option<T>,
}

fn parse_envelope<T: DeserializeOwned>(body: &str, what: &str) -> Result<T, VendorError> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| VendorError::Transient(format!("{what}失败: 响应不是智谱格式: {e}")))?;
    if env.code != 200 {
        let msg = if env.msg.is_empty() {
            format!("code {}", env.code)
        } else {
            env.msg
        };
        return Err(VendorError::Transient(format!("{what}失败: {msg}")));
    }
    env.data
        .ok_or_else(|| VendorError::Transient(format!("{what}失败: 响应缺少 data")))
}

fn build_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Config(format!("创建 HTTP 客户端失败: {e}")))
}

/// 发一次请求，带齐三个鉴权头。HTTP 401/403 视为登录态过期。
async fn send(
    session: &Session,
    request: reqwest::RequestBuilder,
    what: &str,
) -> Result<String, VendorError> {
    let response = request
        .header("authorization", &session.token)
        .header("bigmodel-organization", &session.org)
        .header("bigmodel-project", &session.project)
        .send()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: {e}")))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(VendorError::AuthExpired);
    }
    response
        .text()
        .await
        .map_err(|e| VendorError::Transient(format!("{what}失败: 读取响应出错: {e}")))
}

fn keys_path(session: &Session) -> String {
    format!(
        "/api/biz/v1/organization/{}/projects/{}/api_keys",
        session.org, session.project
    )
}

// ─────────────────────────── key 管理 ───────────────────────────

/// 列表项（实测形状，字段名就是服务端的）。
#[derive(Debug, Deserialize)]
struct KeyItem {
    name: String,
    api_key: String,
    #[serde(default, deserialize_with = "deserialize_iso_time_to_secs")]
    create_time: Option<i64>,
}

/// `2023-09-21T11:00:39.000+08:00` → Unix 秒。解不动就 `None`（展示用的次要信息）。
fn deserialize_iso_time_to_secs<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(&s)
            .map(|t| Some(t.timestamp()))
            .map_err(serde::de::Error::custom),
    }
}

/// ⚠️ [`VendorKey::redacted_key`] 在智谱这边装的是**明文**：列表本身就返回全量
/// `apiKey`，而删除按明文定位（`DELETE …/api_keys/{明文}`）。字段名是 DeepSeek
/// 语义的（脱敏值），这里是同一「删除定位值」槽位的复用。
pub async fn list_keys(session: &Session) -> Result<Vec<VendorKey>, VendorError> {
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    let body = send(
        session,
        client.get(format!("{SITE_ORIGIN}{}?keyType=1", keys_path(session))),
        "拉取密钥列表",
    )
    .await?;
    let items: Vec<KeyItem> = parse_envelope(&body, "拉取密钥列表")?;
    Ok(items
        .into_iter()
        .map(|item| VendorKey {
            name: item.name,
            redacted_key: item.api_key,
            created_at: item.create_time.unwrap_or(0),
            tracking_id: String::new(),
        })
        .collect())
}

/// 建一把新 key，返回**明文**（32 位 hex；校验不含 `*`，防哪天改回脱敏值）。
pub async fn create_key(session: &Session, name: &str) -> Result<String, VendorError> {
    #[derive(Debug, Deserialize)]
    struct Created {
        api_key: String,
    }
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    let body = send(
        session,
        client
            .post(format!("{SITE_ORIGIN}{}", keys_path(session)))
            .json(&serde_json::json!({ "name": name, "keyType": 1 })),
        "创建密钥",
    )
    .await?;
    let data: Created = parse_envelope(&body, "创建密钥")?;
    if data.api_key.is_empty() || data.api_key.contains('*') {
        return Err(VendorError::RedactedValueReturned);
    }
    Ok(data.api_key)
}

/// 删一把 key：`DELETE …/api_keys/{明文}`（定位值在 [`list_keys`] 说的那个槽位里）。
pub async fn delete_key(session: &Session, key: &VendorKey) -> Result<(), VendorError> {
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    let body = send(
        session,
        client.delete(format!(
            "{SITE_ORIGIN}{}/{}",
            keys_path(session),
            key.redacted_key
        )),
        "删除密钥",
    )
    .await?;
    // 响应 data 为 null，要的只是「信封判过」这个副作用。
    let _: serde_json::Value = parse_envelope(&body, "删除密钥")?;
    Ok(())
}

// ─────────────────────────── 余额 ───────────────────────────

#[derive(Debug, Deserialize)]
struct AccountReport {
    balance: Option<f64>,
    available_balance: Option<f64>,
}

/// 查余额（单位：元）。全空 = 没开钱包 → `Ok(None)`，**不是显示 0**。
pub async fn balance(session: &Session) -> Result<Option<UsageResult>, VendorError> {
    let client = build_client().map_err(|e| VendorError::Transient(e.to_string()))?;
    let body = send(
        session,
        client.get(format!(
            "{SITE_ORIGIN}/api/biz/account/query-customer-account-report"
        )),
        "查询余额",
    )
    .await?;
    let data: AccountReport = parse_envelope(&body, "查询余额")?;
    let Some(remaining) = data.available_balance.or(data.balance) else {
        return Ok(None);
    };
    Ok(Some(UsageResult {
        success: true,
        data: Some(vec![crate::provider::UsageData {
            plan_name: Some("钱包余额".to_string()),
            remaining: Some(remaining),
            unit: Some("CNY".to_string()),
            extra: None,
            is_valid: None,
            invalid_message: None,
            total: None,
            used: None,
        }]),
        error: None,
    }))
}

// ─────────────────────── 登录窗（注入脚本 + 凭据回传）───────────────────────

/// 登录窗回传的凭据（回传瞬间从页面上采到的全部材料）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VendorCreds {
    pub token: String,
    pub org: String,
    pub project: String,
    pub account_id: String,
    #[serde(default)]
    pub label: String,
    /// 重登时预填的值（手机号，可能脱敏成 `181****2012` —— 那就当没有）。
    #[serde(default)]
    pub login_identifier: String,
}

/// 判断一次导航是不是凭据回传。`None` = 普通导航（放行）。
pub fn parse_creds_navigation(
    url: &url::Url,
) -> Option<Result<(Session, VendorAccount), AppError>> {
    if url.scheme() != CREDS_SCHEME {
        return None;
    }
    Some(decode_creds(url))
}

fn decode_creds(url: &url::Url) -> Result<(Session, VendorAccount), AppError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    let encoded = url
        .query_pairs()
        .find(|(k, _)| k == "d")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| AppError::Config("凭据回传缺少数据".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .map_err(|e| AppError::Config(format!("凭据回传的数据解不开: {e}")))?;
    let json = String::from_utf8(bytes)
        .map_err(|e| AppError::Config(format!("凭据回传的数据不是 UTF-8: {e}")))?;
    let creds: VendorCreds = serde_json::from_str(&json)
        .map_err(|e| AppError::Config(format!("凭据回传的格式不对: {e}")))?;

    // 三件套缺一 = 后续所有 API 都发不出去，在这里就拒（同 deepseek 的失败语义）。
    if creds.token.is_empty() || creds.org.is_empty() || creds.project.is_empty() {
        return Err(AppError::Config("登录页没有给出完整的智谱登录态".into()));
    }
    if creds.account_id.is_empty() {
        return Err(AppError::Config("登录页没有给出账号标识".into()));
    }

    let session = Session {
        token: creds.token,
        org: creds.org,
        project: creds.project,
    };
    let label = if creds.label.trim().is_empty() {
        creds.account_id.clone()
    } else {
        creds.label.clone()
    };
    Ok((
        session,
        VendorAccount {
            account_id: creds.account_id,
            label,
            login_identifier: creds
                .login_identifier
                .contains('*')
                .then(String::new)
                .unwrap_or(creds.login_identifier),
        },
    ))
}

/// 登录页注入脚本。
///
/// 智谱三种登录方式（微信扫码 / 验证码 / 密码）成功后统一在 `/console/*` 落地，
/// 且三件套（cookie JWT + org + project）落地时机一致 ⇒ **轮询**比劫持响应稳。
/// `login_hint` 是重登预填的手机号（空串不预填）；React 受控组件要派 `input` 事件。
pub fn login_script(login_hint: &str) -> String {
    let hint = serde_json::to_string(login_hint).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"(function () {{
  'use strict';

  if (window.top !== window.self) return;

  var SENT = false;
  var COOKIE_NAME = 'bigmodel_token_production';

  function readCookie(k) {{
    var m = document.cookie.match(new RegExp('(?:^|;\\s)' + k + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  }}
  function readLS(k) {{
    try {{ return window.localStorage.getItem(k); }} catch (e) {{ return null; }}
  }}

  function b64url(s) {{
    var bytes = new TextEncoder().encode(s);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  function trySend() {{
    if (SENT) return;
    var token = readCookie(COOKIE_NAME);
    var org = readLS('Bigmodel-Organization');
    var project = readLS('Bigmodel-Project');
    var userRaw = readLS('user');
    if (!token || !org || !project || !userRaw) return;
    var user;
    try {{ user = JSON.parse(userRaw); }} catch (e) {{ return; }}
    if (!user || !user.id) return;
    SENT = true;
    var payload = {{
      token: token,
      org: org,
      project: project,
      account_id: String(user.id),
      label: user.customerName || user.nickName || '',
      login_identifier: user.phoneNumber || ''
    }};
    window.location.href = '{CREDS_SCHEME}://t?d=' +
      encodeURIComponent(b64url(JSON.stringify(payload)));
  }}

  // 重登预填手机号：只填空框，不覆盖用户已输的内容。
  try {{
    var hint = {hint};
    if (hint) {{
      var el = document.querySelector('input[placeholder*="手机号"]');
      if (el && !el.value) {{
        var setter = Object.getOwnPropertyDescriptor(
          window.HTMLInputElement.prototype, 'value').set;
        setter.call(el, hint);
        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      }}
    }}
  }} catch (e) {{}}

  var timer = setInterval(function () {{
    trySend();
    if (SENT) clearInterval(timer);
  }}, 800);
  setTimeout(function () {{ clearInterval(timer); }}, 20 * 60 * 1000);
}})();
"#
    )
}

// ─────────────────────────── 平台预设 ───────────────────────────

/// `AppType` → `(base_url, model)`。远端 `tier_configs`（键 `bigmodel/{app}`）可覆盖。
pub fn config_for(app: &crate::app_config::AppType) -> Option<(String, String)> {
    let builtin = builtin_config_for(app)?;

    let key = format!("bigmodel/{}", app.as_str());
    let remote = crate::relay::remote_config::load_cached()
        .and_then(|config| config.tier_configs.get(&key).cloned())
        .filter(|config| {
            config.base_url.starts_with("https://") && !config.model.trim().is_empty()
        });

    Some(match remote {
        Some(config) => (config.base_url.clone(), config.model.clone()),
        None => (builtin.0.to_string(), builtin.1.to_string()),
    })
}

fn builtin_config_for(app: &crate::app_config::AppType) -> Option<(&'static str, &'static str)> {
    Some(match app {
        crate::app_config::AppType::Codex => (API_ORIGIN, TURBO),
        crate::app_config::AppType::Claude | crate::app_config::AppType::ClaudeDesktop => {
            (ANTHROPIC_ORIGIN, FLAGSHIP)
        }
        crate::app_config::AppType::Hermes
        | crate::app_config::AppType::OpenClaw
        | crate::app_config::AppType::OpenCode => (API_ORIGIN, FLAGSHIP),
        crate::app_config::AppType::Gemini
        | crate::app_config::AppType::GrokBuild
        | crate::app_config::AppType::CodexImage
        | crate::app_config::AppType::Pi => return None,
    })
}

/// Claude 系四角色 → GLM 模型（与 `deepseek::claude_role_models` 同构）。
/// 远端 `tier_configs` 键 `bigmodel/claude` 的 `claude_roles` 可覆盖。
pub fn claude_role_models() -> crate::relay::provision::ClaudeRoleModels {
    let builtin = crate::relay::provision::ClaudeRoleModels {
        opus: FLAGSHIP.to_string(),
        fable: FLAGSHIP.to_string(),
        sonnet: TURBO.to_string(),
        haiku: TURBO.to_string(),
        subagent: TURBO.to_string(),
    };
    let Some(remote) = crate::relay::remote_config::load_cached()
        .and_then(|config| config.tier_configs.get("bigmodel/claude").cloned())
        .and_then(|config| config.claude_roles)
    else {
        return builtin;
    };

    crate::relay::provision::ClaudeRoleModels {
        opus: remote
            .opus
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(builtin.opus),
        fable: remote
            .fable
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(builtin.fable),
        sonnet: remote
            .sonnet
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(builtin.sonnet),
        haiku: remote
            .haiku
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(builtin.haiku),
        subagent: remote
            .subagent
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(builtin.subagent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与登录脚本 `b64url` 同一套编码（UTF-8 → base64url 无填充）。
    fn creds_url(payload: &serde_json::Value) -> url::Url {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let bytes = serde_json::to_string(payload).unwrap().into_bytes();
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        url::Url::parse(&format!("{CREDS_SCHEME}://t?d={encoded}")).unwrap()
    }

    fn sample_payload() -> serde_json::Value {
        serde_json::json!({
            "token": "eyJhbGciOiJIUzUxMiJ9.payload.sig",
            "org": "org-5c0d8A20C6684935975C765087211AfB",
            "project": "proj_7f70fC4BAb0040879Dc88e2299dB10d4",
            "account_id": "217041",
            "label": "展示智能",
            "login_identifier": "18100002012",
        })
    }

    #[test]
    fn creds_navigation_round_trips_into_session_and_account() {
        let (session, account) = parse_creds_navigation(&creds_url(&sample_payload()))
            .expect("回传导航要被认出")
            .expect("解析要成功");

        assert_eq!(session.token, "eyJhbGciOiJIUzUxMiJ9.payload.sig");
        assert_eq!(session.org, "org-5c0d8A20C6684935975C765087211AfB");
        assert_eq!(account.account_id, "217041");
        assert_eq!(account.label, "展示智能");
        assert_eq!(account.login_identifier, "18100002012");

        // 会话 JSON 要能存进 auth_token 列再解回来（列语义的往返闸）。
        let stored = serde_json::to_string(&session).unwrap();
        assert_eq!(parse_session(&stored).unwrap(), session);
    }

    #[test]
    fn ordinary_navigation_is_passed_through() {
        assert!(parse_creds_navigation(
            &url::Url::parse("https://www.bigmodel.cn/console/overview").unwrap()
        )
        .is_none());
        // 与 deepseek 的回传 scheme 不同名 —— 互不认领。
        assert!(parse_creds_navigation(
            &url::Url::parse("loongport-vendor-creds://t?d=abc").unwrap()
        )
        .is_none());
    }

    #[test]
    fn incomplete_creds_are_rejected_not_stored() {
        for missing in ["token", "org", "project", "account_id"] {
            let mut payload = sample_payload();
            payload[missing] = serde_json::json!("");
            let error = parse_creds_navigation(&creds_url(&payload))
                .expect("回传导航要被认出")
                .expect_err("缺字段必须在这里就拒");
            assert!(
                error.to_string().contains("登录态") || error.to_string().contains("账号"),
                "缺 {missing} 的报错要能定位：{error}"
            );
        }
    }

    #[test]
    fn masked_phone_is_not_kept_as_login_hint() {
        let mut payload = sample_payload();
        payload["login_identifier"] = serde_json::json!("181****2012");
        let (_, account) = parse_creds_navigation(&creds_url(&payload))
            .unwrap()
            .unwrap();
        assert!(
            account.login_identifier.is_empty(),
            "脱敏手机号预填不了任何框，留着只会误导"
        );
    }

    #[test]
    fn envelope_maps_error_code_and_missing_data() {
        assert!(parse_envelope::<serde_json::Value>(
            &serde_json::json!({"code": 200, "data": {}}).to_string(),
            "测试"
        )
        .is_ok());
        assert!(matches!(
            parse_envelope::<serde_json::Value>(
                &serde_json::json!({"code": 500, "msg": "炸了"}).to_string(),
                "测试"
            ),
            Err(VendorError::Transient(ref m)) if m.contains("炸了")
        ));
        assert!(matches!(
            parse_envelope::<serde_json::Value>(
                &serde_json::json!({"code": 200}).to_string(),
                "测试"
            ),
            Err(VendorError::Transient(_))
        ));
    }

    #[test]
    fn keys_path_places_org_and_project_once() {
        let session = Session {
            token: "t".into(),
            org: "org-1".into(),
            project: "proj-2".into(),
        };
        assert_eq!(
            keys_path(&session),
            "/api/biz/v1/organization/org-1/projects/proj-2/api_keys"
        );
    }

    #[test]
    fn builtin_config_covers_six_platforms_with_expected_origins() {
        use crate::app_config::AppType;
        let (codex_base, codex_model) = builtin_config_for(&AppType::Codex).unwrap();
        assert_eq!(codex_base, API_ORIGIN);
        assert_eq!(codex_model, TURBO);

        let (claude_base, _) = builtin_config_for(&AppType::Claude).unwrap();
        assert_eq!(claude_base, ANTHROPIC_ORIGIN);

        assert!(builtin_config_for(&AppType::Gemini).is_none());
        assert!(builtin_config_for(&AppType::OpenCode).is_some());
    }
}
