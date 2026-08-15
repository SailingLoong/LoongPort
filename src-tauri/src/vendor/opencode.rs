//! opencode Zen（opencode.ai）官网直连账号。
//!
//! 与 [`super::deepseek`] / [`super::bigmodel`] 平级并列，但**会话与密钥的采集链
//! 与它们根本不同**，这也是本模块存在的理由：
//!
//! - **会话是 HttpOnly cookie**（`auth`，SolidStart 签名会话）：页面脚本读不到，
//!   只能登录后由 Rust 走原生 `cookies_for_url` 补采（cf_clearance 先例）。
//! - **密钥管理没有稳定 HTTP API**：控制台的 `Key.list` / `Key.create` 走
//!   SolidStart server functions（`/_server` RPC，`x-server-id` 是**前端构建产物
//!   的哈希**，站点一重新构建就变）⇒ Rust 直连不可维护。对策是**登录窗页面上下文
//!   代拉**：注入脚本钩住 `fetch` 记录页面自己的 `/_server` 响应，把窗口带到
//!   `/workspace/{id}/keys`（该页挂载即拉列表，响应含**当前用户 key 的全量明文**），
//!   需要新建时直接驱动页面自己的创建表单 —— hash 换了也照常工作，因为我们
//!   借用的是页面自己的调用。
//! - **key 生命周期在登录时一次完成**：列表 → 按名字认领（`LoongPort专用/a{wrk}`）
//!   → 没有则建 → 明文随登录信号回传落库。之后的 provision 走「本地已有明文 ⇒
//!   零请求」的既有正常路径；`list_keys` / `create_key` / `delete_key` 在 Rust 侧
//!   不可用，返回指路错误（重新登录即重新采集）。
//!
//! ## 账号身份取 workspace id（不是 userID）
//!
//! opencode 的 key **按 workspace 隔离**（`KeyTable.workspaceID`）：同一个人两个
//! workspace 就是两把不相干的 key。所以行的 `account_id` 用 URL 里的
//! `wrk_…`：登录时永远拿得到（不依赖 RPC 响应里有没有行），且与 key 的归属域
//! 严格一致。key 名字跟着它走：`LoongPort专用/awrk_…`（复用
//! [`super::key_name_for`] 的统一公式，跨机认领语义不变）。
//!
//! ## 实测与源码依据
//!
//! 端点、cookie 形态、RPC 响应结构见 design 私仓
//! `vendor-integration/bigmodel-opencode-逆向实录-20260815.md`；`Key.list` /
//! `Key.create` / `remove` 的输入输出与 keys 页「挂载即拉、创建走标准表单」
//! 的行为来自上游开源仓 `packages/console`（`core/src/key.ts`、
//! `app/src/routes/workspace/[id]/keys/key-section.tsx`）。
//!
//! ## 模型目录
//!
//! 端点与模型 ID 对照过公开的 `GET https://opencode.ai/zen/v1/models`
//! （2026-08-16）：Anthropic 兼容 `/zen/v1/messages`（SDK base 取 `/zen`）、
//! OpenAI 兼容 `/zen/v1/chat/completions` 与 Responses `/zen/v1/responses`（base
//! 取 `/zen/v1`）。

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::vendor::{key_name_for, VendorAccount, VendorError};

/// 凭据回传用的自定义 scheme（与 deepseek / bigmodel 不同名，互不认领）。
const CREDS_SCHEME: &str = "loongport-opencode-creds";

/// 登录窗 label（各厂商的登录窗可以同时开）。
pub const LOGIN_WINDOW_LABEL: &str = "loongport-opencode-login";

pub const SITE_ORIGIN: &str = "https://opencode.ai";

/// 功能性登录页（GitHub / Google OAuth）。邀请链接（`?ref=…`）由远端配置覆盖。
pub const LOGIN_URL: &str = "https://opencode.ai/auth";

/// OpenAI 兼容 API 根（chat/completions 与 Responses 都挂在它下面）。
pub const API_ORIGIN: &str = "https://opencode.ai/zen/v1";

/// Anthropic 兼容层（`/zen/v1/messages` ⇒ SDK base_url 取 `/zen`）。
pub const ANTHROPIC_ORIGIN: &str = "https://opencode.ai/zen";

/// 会话 cookie 名（console `useSession({ name: "auth" })`，HttpOnly）。
const SESSION_COOKIE_NAME: &str = "auth";

// ─────────────────────── 会话凭据（auth_token 列的形态） ───────────────────────

/// 调用厂商侧所必需的凭据，JSON 存进 `auth_token` 列。
///
/// `cookie` 是 HttpOnly 会话的原值 —— 只在**密钥采集**时有用（借登录窗页面），
/// API 调用本身吃 `api_key`，所以它过期不影响已 provision 的 provider。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub cookie: String,
    pub workspace_id: String,
}

/// 读回 `auth_token` 列。opencode 的 key 管理走登录窗，生产侧暂无调用方；
/// 保留它钉住列的存储形态（测试拿它做往返闸），上游将来出余额 API 时就是现成读端。
#[allow(dead_code)]
pub fn parse_session(auth_token: &str) -> Result<Session, AppError> {
    if auth_token.trim().is_empty() {
        return Err(AppError::Config("这个官网账号没有登录态".into()));
    }
    serde_json::from_str(auth_token)
        .map_err(|e| AppError::Config(format!("opencode 登录态格式不对: {e}")))
}

// ─────────────────────── 登录信号（页面脚本回传的载荷） ───────────────────────

/// 登录窗脚本回传的采集结果。**不是**最终存库形态 —— cookie 页面读不到，
/// 由 [`compose_session`] 在 Rust 侧补齐成 [`Session`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginSignal {
    pub workspace_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub keys: Vec<HarvestedKey>,
}

/// 采到的一把明文 key。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestedKey {
    pub name: String,
    pub key: String,
}

/// 判断一次导航是不是 opencode 的登录信号回传。`None` = 普通导航（放行）。
///
/// 返回的 [`crate::vendor::VendorSession`] 里 `auth_token` 暂存信号 JSON，
/// 等 [`compose_session`] 换成最终 [`Session`]（cookie 只能原生补采，见模块文档）。
pub fn parse_creds_navigation(
    url: &url::Url,
) -> Option<Result<crate::vendor::VendorSession, AppError>> {
    if url.scheme() != CREDS_SCHEME {
        return None;
    }
    Some(decode_signal(url))
}

fn decode_signal(url: &url::Url) -> Result<crate::vendor::VendorSession, AppError> {
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
    let mut signal: LoginSignal = serde_json::from_str(&json)
        .map_err(|e| AppError::Config(format!("凭据回传的格式不对: {e}")))?;

    if !signal.workspace_id.starts_with("wrk_") || signal.workspace_id.len() <= 4 {
        return Err(AppError::Config(
            "登录页没有给出有效的 workspace 标识".into(),
        ));
    }
    // 只有名字没有明文的行（别人的 key / 采残了）对认领毫无用处，就地丢弃。
    signal
        .keys
        .retain(|k| !k.name.is_empty() && !k.key.is_empty());

    let label = if signal.email.trim().is_empty() {
        signal.workspace_id.clone()
    } else {
        signal.email.clone()
    };
    let account = VendorAccount {
        account_id: signal.workspace_id.clone(),
        label,
        // OAuth 登录没有可预填的输入框。
        login_identifier: String::new(),
    };
    Ok(crate::vendor::VendorSession {
        auth_token: serde_json::to_string(&signal)
            .map_err(|e| AppError::Config(format!("登录信号编码失败: {e}")))?,
        account,
    })
}

/// 从窗口 cookie 里抽出 opencode 会话（`auth`）。`None` = 没拿到（报错由调用方定）。
pub fn extract_session_cookie(cookies: &[tauri::webview::Cookie<'_>]) -> Option<String> {
    cookies
        .iter()
        .find(|cookie| cookie.name() == SESSION_COOKIE_NAME && !cookie.value().trim().is_empty())
        .map(|cookie| cookie.value().to_string())
}

/// 把登录信号 + 原生采到的 cookie 合成最终会话，并顺手认领本客户端那把 key。
///
/// 返回 `(auth_token 列的最终 JSON, 采到的明文 key)`。key 按统一命名公式认领
/// （`LoongPort专用/a{workspace_id}`）；没有就 `None`（行以「未获取密钥」存在，
/// 用户重登一次即可重新采集）。
pub fn compose_session(
    cookie: Option<String>,
    signal_json: &str,
) -> Result<(String, Option<String>), AppError> {
    let signal: LoginSignal = serde_json::from_str(signal_json)
        .map_err(|e| AppError::Config(format!("opencode 登录信号格式不对: {e}")))?;
    let Some(cookie) = cookie else {
        return Err(AppError::Config(
            "没取到 opencode 登录会话（auth cookie），请重试登录".into(),
        ));
    };
    let session = Session {
        cookie,
        workspace_id: signal.workspace_id.clone(),
    };
    let wanted = key_name_for(&signal.workspace_id);
    let claimed = signal
        .keys
        .iter()
        .find(|k| k.name == wanted && !k.key.contains('*'))
        .map(|k| k.key.clone());
    let token = serde_json::to_string(&session)
        .map_err(|e| AppError::Config(format!("opencode 会话编码失败: {e}")))?;
    Ok((token, claimed))
}

// ─────────────────── key 管理：Rust 侧不可用（见模块文档） ───────────────────

fn in_window_only(what: &str) -> VendorError {
    VendorError::Transient(format!(
        "{what}走的是登录窗页面采集，Rust 侧没有稳定接口：请在官方 API 页重新登录该账号，登录时会自动认领或创建密钥"
    ))
}

pub async fn list_keys(_auth_token: &str) -> Result<Vec<crate::vendor::VendorKey>, VendorError> {
    Err(in_window_only("拉取 opencode 密钥列表"))
}

pub async fn create_key(_auth_token: &str, _name: &str) -> Result<String, VendorError> {
    Err(in_window_only("创建 opencode 密钥"))
}

pub async fn delete_key(
    _auth_token: &str,
    _key: &crate::vendor::VendorKey,
) -> Result<(), VendorError> {
    Err(in_window_only("删除 opencode 密钥"))
}

/// opencode 没有公开的余额 API（上游 issue #10448 仍是个 feature request）。
/// `Ok(None)` = 「没有可展示的余额」，与 bigmodel「没开钱包」同一语义。
pub async fn balance() -> Result<Option<crate::provider::UsageResult>, VendorError> {
    Ok(None)
}

// ─────────────────────── 登录窗（注入脚本） ───────────────────────

/// 登录页注入脚本。`login_hint` 对 OAuth 无用（没有可预填的框），签名与兄弟厂商对齐。
///
/// 状态机（全在页面侧，幂等于 pathname）：
///
/// 1. 落在 `/workspace/wrk_…`（OAuth 回跳、含 workspace 选择器跳转后的落点）
///    ⇒ `location.assign` 到 `/workspace/{wrk}/keys`（该页挂载即拉 key 列表）。
/// 2. keys 页上等 fetch 钩子记到含 `keyDisplay` 的 `/_server` 响应：
///    - 有 `LoongPort专用/a{wrk}` 那把（跨机认领）⇒ 立即回传；
///    - 没有 ⇒ 驱动页面自己的创建表单（点创建、填名字、提交），等新捕获；
///    - 自动化失败 ⇒ 页面浮条请用户手动创建（任意名字），新出现的 key 也收；
///    - 两分钟兜底：按现状回传（可能没有 key，行以「未获取密钥」存在）。
///
/// fetch 钩子只记录 `/_server` 响应文本 —— 明文 key 与 workspace 内的 email
/// 都在列表响应里，解析优先 `JSON.parse`、退正则（solid-start 的序列化近似 JSON）。
pub fn login_script(_login_hint: &str) -> String {
    format!(
        r#"(function () {{
  'use strict';

  if (window.top !== window.self) return;
  // OAuth 在 auth.opencode.ai / github.com / google 上跳，别去那边装钩子。
  if (window.location.host !== 'opencode.ai') return;

  var S = (window.__loongport_oc = window.__loongport_oc || {{ log: [], sent: false }});
  if (!S.hooked) {{
    S.hooked = true;
    var orig = window.fetch;
    window.fetch = function (input, init) {{
      var url = typeof input === 'string' ? input : (input && input.url) || '';
      var p = orig.apply(this, arguments);
      if (String(url).indexOf('/_server') !== -1) {{
        p.then(function (r) {{
          r.clone().text().then(function (t) {{ S.log.push(t); }}).catch(function () {{}});
        }}).catch(function () {{}});
      }}
      return p;
    }};
  }}
  if (S.sent) return;

  var m = window.location.pathname.match(/^\/workspace\/(wrk_[A-Za-z0-9]+)/);
  if (!m) return;
  var wrk = m[1];
  var KEY_NAME = 'LoongPort专用/a' + wrk;

  // ── 第一步：把窗口带到 keys 页（那里挂载即拉列表） ──
  if (window.location.pathname.indexOf('/keys') === -1) {{
    window.location.assign('/workspace/' + wrk + '/keys');
    return;
  }}

  function b64url(s) {{
    var bytes = new TextEncoder().encode(s);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  function send(keys, email) {{
    if (S.sent) return;
    S.sent = true;
    var payload = {{
      workspace_id: wrk,
      email: email || '',
      keys: keys.map(function (k) {{ return {{ name: k.name, key: k.key }}; }})
    }};
    window.location.href = '{CREDS_SCHEME}://t?d=' + encodeURIComponent(b64url(JSON.stringify(payload)));
  }}

  // 解析一段 /_server 响应文本：优先 JSON.parse（含 superjson 的 {{json:[…]}} 形态），
  // 退正则。行内字段序为 id,name,key,…,email,keyDisplay（console Key.list 的 SELECT+map）。
  function parseBody(body) {{
    var out = [];
    var email = '';
    var hit = body.indexOf('keyDisplay') !== -1;
    var rows = null;
    try {{
      var parsed = JSON.parse(body);
      if (Object.prototype.toString.call(parsed) === '[object Array]') rows = parsed;
      else if (parsed && typeof parsed === 'object') {{
        rows = parsed.json || parsed.result || parsed.data || null;
        if (Object.prototype.toString.call(rows) !== '[object Array]') rows = null;
      }}
    }} catch (e) {{ rows = null; }}
    if (rows) {{
      for (var i = 0; i < rows.length; i++) {{
        var r = rows[i];
        if (!r || typeof r !== 'object' || !('keyDisplay' in r)) continue;
        if (r.key) out.push({{ name: String(r.name || ''), key: String(r.key) }});
        if (r.email && !email) email = String(r.email);
      }}
      return {{ keys: out, email: email, hit: true }};
    }}
    var re = /"name":"((?:[^"\\]|\\.)*)"[^{{}}]*?"key":"(sk-[A-Za-z0-9]+)"/g;
    var mm;
    while ((mm = re.exec(body)) !== null) out.push({{ name: mm[1], key: mm[2] }});
    var em = body.match(/"email":"([^"]+@[^"]+)"/);
    if (em) email = em[1];
    return {{ keys: out, email: email, hit: hit }};
  }}

  function collected() {{
    var keys = [];
    var email = '';
    for (var i = 0; i < S.log.length; i++) {{
      var r = parseBody(S.log[i]);
      for (var j = 0; j < r.keys.length; j++) keys.push(r.keys[j]);
      if (!email && r.email) email = r.email;
    }}
    return {{ keys: keys, email: email }};
  }}

  function ours(list) {{
    for (var i = 0; i < list.length; i++) {{
      if (list[i].name === KEY_NAME && list[i].key) return list[i];
    }}
    return null;
  }}

  function tryCreate() {{
    try {{
      var btn = document.querySelector('[data-slot="title-row"] button[data-color="primary"]');
      if (!btn) return false;
      btn.click();
      var input = document.querySelector('form input[name="name"]');
      if (!input) return false;
      var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
      setter.call(input, KEY_NAME);
      input.dispatchEvent(new Event('input', {{ bubbles: true }}));
      var submit = document.querySelector('form button[type="submit"]');
      if (!submit) return false;
      submit.click();
      return true;
    }} catch (e) {{ return false; }}
  }}

  function banner(text) {{
    try {{
      var el = document.createElement('div');
      el.textContent = text;
      el.style.cssText = 'position:fixed;left:50%;bottom:24px;transform:translateX(-50%);z-index:2147483647;background:#1f2937;color:#fff;padding:10px 16px;border-radius:8px;font:13px/1.5 system-ui,sans-serif;max-width:80vw;box-shadow:0 4px 16px rgba(0,0,0,.35)';
      document.body.appendChild(el);
      setTimeout(function () {{ if (el.parentNode) el.parentNode.removeChild(el); }}, 60000);
    }} catch (e) {{}}
  }}

  var started = Date.now();
  var firstNames = null;
  var createTried = false;

  var timer = setInterval(function () {{
    if (S.sent) {{ clearInterval(timer); return; }}
    var cur = collected();
    var mine = ours(cur.keys);

    // A：已有一把我们的（别的机器建的，认领路径）⇒ 立即回传。
    if (mine) {{ send([mine], cur.email); clearInterval(timer); return; }}

    var listArrived = false;
    for (var i = 0; i < S.log.length; i++) {{
      if (S.log[i].indexOf('keyDisplay') !== -1) {{ listArrived = true; break; }}
    }}
    if (!listArrived) {{
      if (Date.now() - started > 120000) {{ send(cur.keys, cur.email); clearInterval(timer); }}
      return;
    }}
    if (!firstNames) {{
      firstNames = cur.keys.map(function (k) {{ return k.name; }});
    }}

    // B：驱动页面自己的创建表单；失败就请用户手动建（任意名字都收）。
    if (!createTried) {{
      createTried = true;
      if (!tryCreate()) {{
        banner('LoongPort：自动创建密钥没有成功，请在页面上点 Create key 手动创建一把（名字随意），创建后会自动完成接入');
      }}
      return;
    }}

    // C：我们的那把，或快照之后新出现的任意一把（手动兜底路径）。
    for (var i = 0; i < cur.keys.length; i++) {{
      var k = cur.keys[i];
      if (k.name === KEY_NAME || firstNames.indexOf(k.name) === -1) {{
        send([k], cur.email);
        clearInterval(timer);
        return;
      }}
    }}

    // D：两分钟兜底 —— 按现状回传（可能没有 key；行会以「未获取密钥」存在）。
    if (Date.now() - started > 120000) {{
      send(cur.keys, cur.email);
      clearInterval(timer);
    }}
  }}, 700);
}})();
"#
    )
}

// ─────────────────────── 平台预设 ───────────────────────

/// Claude 系默认主模型（coding 甜点档）。
const FLAGSHIP: &str = "claude-sonnet-5";
/// Claude 系便宜档。
const HAIKU: &str = "claude-haiku-4-5";
/// Codex 走 Responses 端点，用 codex 系模型。
const CODEX_MODEL: &str = "gpt-5.3-codex";
/// OpenAI 兼容（chat/completions）平台的主模型。
const CHAT_MODEL: &str = "deepseek-v4-pro";

/// `AppType` → `(base_url, model)`。远端 `tier_configs`（键 `opencode/{app}`）可覆盖。
pub fn config_for(app: &crate::app_config::AppType) -> Option<(String, String)> {
    let builtin = builtin_config_for(app)?;

    let key = format!("opencode/{}", app.as_str());
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
        crate::app_config::AppType::Codex => (API_ORIGIN, CODEX_MODEL),
        crate::app_config::AppType::Claude | crate::app_config::AppType::ClaudeDesktop => {
            (ANTHROPIC_ORIGIN, FLAGSHIP)
        }
        crate::app_config::AppType::Hermes
        | crate::app_config::AppType::OpenClaw
        | crate::app_config::AppType::OpenCode => (API_ORIGIN, CHAT_MODEL),
        crate::app_config::AppType::Gemini
        | crate::app_config::AppType::GrokBuild
        | crate::app_config::AppType::CodexImage
        | crate::app_config::AppType::Pi => return None,
    })
}

/// Claude 系四角色 → zen 的 Claude 模型。远端 `tier_configs` 键
/// `opencode/claude` 的 `claude_roles` 可覆盖。
pub fn claude_role_models() -> crate::relay::provision::ClaudeRoleModels {
    let builtin = crate::relay::provision::ClaudeRoleModels {
        opus: "claude-opus-5".to_string(),
        fable: "claude-fable-5".to_string(),
        sonnet: FLAGSHIP.to_string(),
        haiku: HAIKU.to_string(),
        subagent: HAIKU.to_string(),
    };
    let Some(remote) = crate::relay::remote_config::load_cached()
        .and_then(|config| config.tier_configs.get("opencode/claude").cloned())
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

    fn sample_signal() -> serde_json::Value {
        serde_json::json!({
            "workspace_id": "wrk_abc123",
            "email": "dev@example.com",
            "keys": [
                { "name": "LoongPort专用/awrk_abc123", "key": "sk-plain-texthex" },
                { "name": "my own key", "key": "sk-another-key00" }
            ]
        })
    }

    #[test]
    fn signal_round_trips_into_account_and_composable_session() {
        let crate::vendor::VendorSession {
            auth_token,
            account,
        } = parse_creds_navigation(&creds_url(&sample_signal()))
            .expect("回传导航要被认出")
            .expect("解析要成功");

        assert_eq!(account.account_id, "wrk_abc123");
        assert_eq!(account.label, "dev@example.com");
        assert!(account.login_identifier.is_empty());

        // compose 补上 cookie 后：auth_token 换成最终 Session 形态，并认领我们的 key。
        let (token, claimed) =
            compose_session(Some("signed-session-cookie".into()), &auth_token).unwrap();
        let session = parse_session(&token).unwrap();
        assert_eq!(session.cookie, "signed-session-cookie");
        assert_eq!(session.workspace_id, "wrk_abc123");
        assert_eq!(claimed.as_deref(), Some("sk-plain-texthex"));
    }

    #[test]
    fn missing_cookie_is_an_error_not_a_silent_empty_session() {
        let crate::vendor::VendorSession { auth_token, .. } =
            parse_creds_navigation(&creds_url(&sample_signal()))
                .unwrap()
                .unwrap();
        assert!(compose_session(None, &auth_token).is_err());
    }

    #[test]
    fn non_workspace_ids_are_rejected() {
        for bad in ["", "usr_someone", "wrk_"] {
            let mut payload = sample_signal();
            payload["workspace_id"] = serde_json::json!(bad);
            let result = parse_creds_navigation(&creds_url(&payload));
            if bad.is_empty() {
                // workspace_id 缺失时 serde 直接报格式错 —— 也算拒绝。
                assert!(result.unwrap().is_err());
            } else {
                assert!(
                    result.unwrap().is_err(),
                    "{bad} 不是 workspace 标识，必须在这里就拒"
                );
            }
        }
    }

    #[test]
    fn keyless_rows_are_dropped_and_fall_back_to_workspace_label() {
        let mut payload = sample_signal();
        payload["keys"] = serde_json::json!([{ "name": "masked-only", "key": "" }]);
        payload["email"] = serde_json::json!("");
        let crate::vendor::VendorSession {
            auth_token,
            account,
        } = parse_creds_navigation(&creds_url(&payload))
            .unwrap()
            .unwrap();
        assert_eq!(
            account.label, "wrk_abc123",
            "没采到 email 就回落 workspace id"
        );

        let (_, claimed) = compose_session(Some("c".into()), &auth_token).unwrap();
        assert_eq!(claimed, None, "没有我们的 key 就不认领，不猜");
    }

    #[test]
    fn ordinary_navigation_is_passed_through() {
        assert!(parse_creds_navigation(
            &url::Url::parse("https://opencode.ai/workspace/wrk_x/keys").unwrap()
        )
        .is_none());
        assert!(parse_creds_navigation(
            &url::Url::parse("loongport-vendor-creds://t?d=abc").unwrap()
        )
        .is_none());
    }

    #[test]
    fn script_navigates_to_keys_and_hooks_server_rpc() {
        let script = login_script("");
        for needle in [
            "opencode.ai",
            "/workspace/' + wrk + '/keys",
            "'/_server'",
            "LoongPort专用/a' + wrk",
            "input[name=\"name\"]",
            "button[type=\"submit\"]",
        ] {
            assert!(
                script.contains(needle),
                "登录脚本缺关键片段：{needle}（改脚本时同步改这条闸）"
            );
        }
    }

    #[tokio::test]
    async fn key_management_from_rust_is_expected_to_fail_with_guidance() {
        let list_err = list_keys("t").await.expect_err("Rust 侧拉列表必须失败");
        let create_err = create_key("t", "n")
            .await
            .expect_err("Rust 侧建 key 必须失败");
        for msg in [format!("{list_err:?}"), format!("{create_err:?}")] {
            assert!(msg.contains("重新登录"), "错误要指路：{msg}");
        }
    }

    #[tokio::test]
    async fn balance_reports_nothing_to_show() {
        // Ok(None) = 没有可展示的余额（上游无公开 API），不是错误。
        assert!(balance().await.unwrap().is_none());
    }

    #[test]
    fn builtin_config_covers_six_platforms_with_expected_origins() {
        use crate::app_config::AppType;
        let (codex_base, codex_model) = builtin_config_for(&AppType::Codex).unwrap();
        assert_eq!(codex_base, API_ORIGIN);
        assert_eq!(codex_model, CODEX_MODEL);

        let (claude_base, _) = builtin_config_for(&AppType::Claude).unwrap();
        assert_eq!(claude_base, ANTHROPIC_ORIGIN);

        assert!(builtin_config_for(&AppType::Gemini).is_none());
        assert!(builtin_config_for(&AppType::OpenCode).is_some());
    }
}
