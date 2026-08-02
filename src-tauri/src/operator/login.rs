//! 登录 WebView：加载运营商真实登录页，登录成功后把 localStorage 里的凭据回传原生侧。
//!
//! ## 凭据怎么回来的
//!
//! Tauri 的 `eval` 不回传返回值，而给远端页面开 IPC 需要 `remote` capability（等于让运营商
//! 页面能调本地命令，不能开）。所以走**导航拦截**：
//!
//! 1. 注入脚本（`initialization_script`）在登录页里跑，劫持 `localStorage.setItem`；
//! 2. 四个凭据键齐了之后，脚本发起一次跳转到 `loongport-creds://ok?d=<base64url>`；
//! 3. Rust 侧 `on_navigation` 回调收到这个 URL，解出凭据，返回 `false` 拦下跳转。
//!
//! **与 V1 的差异**：V1 走 `document.title` 分片协议（LP1 握手 + stop-and-wait + FNV 校验 +
//! 重传，Rust 695 行 + JS 460 行），因为 Windows WebView2 的标题上限是 4096 字符，装不下
//! 一份完整凭据。V2 只做 macOS，URL 长度上限远高于此，一次跳转就送完 —— 不需要分片、握手、
//! 重传、校验和。
//!
//! ⚠️ **要加 Windows 时先测这里**：WebView2 对自定义 scheme 的导航拦截行为与 WKWebView
//! 不同，URL 长度上限也另有其数。若拦不住或装不下，回退方案就是 V1 那套分片协议
//! （`LoongPort/src-tauri/src/operator/{title_channel.rs,login_script.js}`）。
//!
//! ## 为什么加载 `/login` 而不是 `/register`
//!
//! sub2api 的 `/register` 在运营商关闭注册时**不跳走**，只把表单换成一条黄条 —— 那是个死页。
//! `/login` 页脚有「去注册」链接，注册开着时用户点一下就到，关着时用户看到的是正常的登录页。

use serde::Serialize;

use crate::error::AppError;

/// 登录窗口的 label。
///
/// **不得与 `tauri.conf.json` 的 `app.windows` 里任何 label 重名** —— 两边都声明同一个
/// label 会在 setup hook 里 panic（`a webview with label "..." already exists`）。
pub const LOGIN_WINDOW_LABEL: &str = "loongport-login";

/// 凭据回传用的自定义 scheme。
///
/// 选一个绝不会被真实网站用到的值：注入脚本只在运营商 origin 下跑，但万一页面自己跳到某个
/// 我们认得的 scheme，就会被误当成凭据回传。
const CREDS_SCHEME: &str = "loongport-creds";

/// 登录 WebView 的 User-Agent。
///
/// **Rust 侧的 HTTP 客户端必须用同一个值**（见 [`crate::operator::api::Client`]）：sub2api
/// 有可选的会话绑定（默认关），开启后 access token 里带 `SHA256(clientIP + "\n" + UA)[:16]`，
/// UA 不一致会 401 且**撤销整个会话家族**（连网页登录态一起踢）。
///
/// 显式写死而不是让两边各用默认值：WKWebView 与 reqwest 的默认 UA 天差地别，靠默认值必然
/// 不一致。写死之后两边引用同一个常量，改一处两边都跟。
pub const WEBVIEW_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 LoongPort";

/// 从登录页取回的凭据。
#[derive(Debug, Clone, Serialize)]
pub struct Credentials {
    pub auth_token: String,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<i64>,
}

/// 凭据到手后浮在登录页顶部的提示条。
///
/// 存在的理由：**窗口不再自动关闭**（见 `commands::operator::do_login` 里那段说明），所以
/// 必须告诉用户「这边已经好了，你可以继续用这个页面，也可以关掉它」—— 否则他不知道该干什么，
/// 只会盯着一个看起来没反应的窗口。
///
/// 写成一整段自执行 JS 常量而不是模板：它没有任何需要插值的东西，做成 `format!` 只会引入
/// 转义风险。
///
/// 用 `position: fixed` + 高 `z-index`，不改页面自身的任何 DOM 结构 —— 运营商的页面长什么样
/// 我们不该假设，只在最上层贴一条。
pub const CONNECTED_BANNER_JS: &str = r#"(function () {
  var ID = '__loongport_connected__';
  if (document.getElementById(ID)) return;

  var bar = document.createElement('div');
  bar.id = ID;
  bar.setAttribute('style', [
    'position:fixed', 'top:0', 'left:0', 'right:0', 'z-index:2147483647',
    'padding:10px 14px', 'background:#10b981', 'color:#fff',
    'font:500 13px/1.5 -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif',
    'display:flex', 'align-items:center', 'gap:10px',
    'box-shadow:0 1px 6px rgba(0,0,0,.2)'
  ].join(';'));

  var text = document.createElement('span');
  text.style.flex = '1';
  text.textContent = 'LoongPort 已连接，正在准备密钥。你可以继续在这里充值或查看用量，用完直接关掉此窗口。';
  bar.appendChild(text);

  var btn = document.createElement('button');
  btn.textContent = '知道了';
  btn.setAttribute('style', [
    'flex:none', 'cursor:pointer', 'border:0', 'border-radius:4px',
    'padding:4px 10px', 'background:rgba(255,255,255,.22)', 'color:#fff',
    'font:inherit'
  ].join(';'));
  btn.onclick = function () { bar.remove(); };
  bar.appendChild(btn);

  (document.body || document.documentElement).appendChild(bar);
})();
"#;

/// 生成注入脚本。
///
/// `site_origin` 用于 origin 守卫：脚本只在运营商自己的页面上生效，跳到第三方（OAuth 授权页
/// 之类）时不执行 —— 那些页面的 localStorage 里没有我们要的东西，读它只是徒增攻击面。
///
/// `login_identifier` 是重登时预填进登录框的值（空串 = 不预填）。
///
/// ## 预填为什么要派 `input` 事件
///
/// sub2api 的登录页是 Vue（`LoginView.vue`，输入框 `id="email"` + `v-model="formData.email"`）。
/// 只设 `input.value` 的话 DOM 上看得见字、但 `formData.email` 仍是空的 —— 提交上去是空邮箱。
/// `v-model` 监听的是 `input` 事件，所以必须派一个 `bubbles: true` 的 `Event('input')`
/// 让框架的监听器收到。
///
/// **只填不提交**：密码与人机验证（Turnstile）都得用户自己来，这里只省掉输邮箱那一步。
pub fn login_script(site_origin: &str, login_identifier: &str) -> String {
    // JSON 编码 origin 而不是直接插进单引号里：origin 来自用户输入，含引号就会破坏脚本语法。
    let origin_literal = serde_json::to_string(site_origin).unwrap_or_else(|_| "\"\"".to_string());
    // 同理 JSON 编码：这个值来自服务端返回的账号信息，含引号就会破坏脚本语法。
    let identifier_literal =
        serde_json::to_string(login_identifier).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"(function () {{
  'use strict';

  // 只在顶层 frame 跑：同源 iframe 会让脚本多执行一份、重复回传。
  if (window.top !== window.self) return;

  var ALLOWED_ORIGIN = {origin_literal};
  if (window.location.origin !== ALLOWED_ORIGIN) return;

  // 重登时把登录标识填回去，用户只需补密码与人机验证。空串 = 首次登录，不填。
  var PREFILL = {identifier_literal};

  // 只认登录标识那个框，别碰密码框。
  //
  // 选择器按「最稳」到「最泛」排：sub2api 的框是 id="email" + type="email"
  // （LoginView.vue 实测），new-api 那类可能叫 username —— 都试一遍，命中第一个就停。
  // 不用 [name=...]：Vue 的 v-model 不要求写 name，实测那个框就没有。
  var PREFILL_SELECTORS = [
    '#email',
    'input[type=email]',
    'input[autocomplete=email]',
    '#username',
    'input[autocomplete=username]'
  ];

  var prefilled = false;

  function tryPrefill() {{
    if (prefilled || !PREFILL) return;
    for (var i = 0; i < PREFILL_SELECTORS.length; i++) {{
      var el = document.querySelector(PREFILL_SELECTORS[i]);
      if (!el) continue;
      // 用户已经自己输了东西就别覆盖 —— 他可能正要换个账号登。
      if (el.value) {{ prefilled = true; return; }}
      el.value = PREFILL;
      // **必须派事件**：只设 .value 的话 DOM 上有字但 Vue 的 formData 还是空的，
      // 提交上去就是空邮箱。v-model 听的是 input 事件。
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      el.dispatchEvent(new Event('change', {{ bubbles: true }}));
      prefilled = true;
      return;
    }}
  }}

  // sub2api 前端的凭据键名（frontend/src/stores/auth.ts）。
  var K_TOKEN = 'auth_token';
  var K_REFRESH = 'refresh_token';
  var K_EXPIRES = 'token_expires_at';

  var sent = false;

  function b64url(s) {{
    var bytes = new TextEncoder().encode(s);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  function trySend() {{
    if (sent) return;
    var token = null;
    try {{ token = window.localStorage.getItem(K_TOKEN); }} catch (e) {{ return; }}
    // access token 是唯一的必需项。refresh / expires 缺失是服务端的已知降级态
    // （可用但不可续期），不能因此判成「还没登录完」去无限等待。
    if (!token) return;

    sent = true;
    var payload = JSON.stringify({{
      auth_token: token,
      refresh_token: window.localStorage.getItem(K_REFRESH),
      token_expires_at: window.localStorage.getItem(K_EXPIRES)
    }});

    // 改 window.location 发这次跳转。
    //
    // **不能用隐藏 iframe**（试过，会被 CSP 拦掉）：sub2api 全站带
    // `frame-src challenges.cloudflare.com https://*.stripe.com ...` 的白名单，
    // `{CREDS_SCHEME}://` 不在其中，iframe 的 src 直接被浏览器阻断，凭据根本发不出去。
    // 同一条 CSP 还有 `frame-ancestors 'none'` 与 `X-Frame-Options: DENY`。
    //
    // 顶层导航则不受这条 CSP 管（它没有 `navigate-to` 指令），而且 `on_navigation` 返回
    // false 会**拦下**这次导航 —— 页面不会真的走掉，用户看到的内容原样留着。
    window.location.href = '{CREDS_SCHEME}://ok?d=' + b64url(payload);
  }}

  // 劫持 setItem：登录成功的那一刻四个键会陆续写进来。
  // 用 setTimeout(0) 而不是当场读 —— 邮密登录与 OAuth 登录的落盘顺序不同，
  // 当场读会在某条路径上抢跑（拿到只有 token 没有 expires 的半份）。
  // 延到同步段跑完再读一次性拿全，两条路径的结果就一致了。
  try {{
    var orig = window.localStorage.setItem.bind(window.localStorage);
    window.localStorage.setItem = function (k, v) {{
      orig(k, v);
      if (k === K_TOKEN || k === K_REFRESH || k === K_EXPIRES) {{
        setTimeout(trySend, 0);
      }}
    }};
  }} catch (e) {{ /* 存储不可用时下面的轮询兜底 */ }}

  // 兜底：用户可能是「已登录状态」直接打开页面（localStorage 里本来就有 token，
  // 不会再触发 setItem）。轮询到拿到为止，最多 5 分钟。
  var polls = 0;
  var timer = setInterval(function () {{
    polls++;
    trySend();
    // 预填搭同一个轮询：SPA 的登录表单是异步渲染的，脚本跑的时候那个框往往还不存在。
    tryPrefill();
    if (sent || polls > 600) clearInterval(timer);
  }}, 500);
  trySend();
  tryPrefill();
}})();
"#
    )
}

/// 登录页 URL。
pub fn login_url(site_origin: &str) -> String {
    format!("{site_origin}/login")
}

/// 判断一次导航是不是凭据回传，是则解出凭据。
///
/// 返回 `None` 表示这是普通导航（放行）；返回 `Some` 表示这是回传（调用方应拦下）。
pub fn parse_creds_navigation(url: &url::Url) -> Option<Result<Credentials, AppError>> {
    if url.scheme() != CREDS_SCHEME {
        return None;
    }
    Some(decode_creds(url))
}

fn decode_creds(url: &url::Url) -> Result<Credentials, AppError> {
    let encoded = url
        .query_pairs()
        .find(|(k, _)| k == "d")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| AppError::Config("凭据回传缺少数据".into()))?;

    let json =
        decode_b64url(&encoded).ok_or_else(|| AppError::Config("凭据回传的数据解不开".into()))?;

    #[derive(serde::Deserialize)]
    struct Raw {
        auth_token: Option<String>,
        refresh_token: Option<String>,
        token_expires_at: Option<String>,
    }
    let raw: Raw = serde_json::from_str(&json)
        .map_err(|e| AppError::Config(format!("凭据回传的格式不对: {e}")))?;

    let auth_token = raw
        .auth_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Config("登录页没有给出 access token".into()))?;

    Ok(Credentials {
        auth_token,
        refresh_token: raw.refresh_token.filter(|t| !t.is_empty()),
        // sub2api 存的是毫秒时间戳字符串。解不出来不算错 —— 那就是「可用但不知何时过期」，
        // 与「没登录」是两件事，不该因此把整份凭据扔掉。
        token_expires_at: raw
            .token_expires_at
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(|ms| if ms > 100_000_000_000 { ms / 1000 } else { ms }),
    })
}

fn decode_b64url(s: &str) -> Option<String> {
    // 手写而不引 base64 crate：只用在这一处，且输入是我们自己的脚本产生的。
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }

    let mut bits = 0u32;
    let mut nbits = 0u32;
    let mut out = Vec::new();
    for &b in s.as_bytes() {
        if b == b'=' {
            break;
        }
        let v = lookup[b as usize];
        if v == 255 {
            return None;
        }
        bits = (bits << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds_url(payload: &str) -> url::Url {
        let mut bin = String::new();
        // 复用生产解码器的逆运算，避免测试自带一份可能与生产不一致的编码实现。
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = payload.as_bytes();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
            let keep = chunk.len() + 1;
            for &i in idx.iter().take(keep) {
                bin.push(TABLE[i as usize] as char);
            }
        }
        url::Url::parse(&format!("{CREDS_SCHEME}://ok?d={bin}")).unwrap()
    }

    #[test]
    fn ordinary_navigation_is_not_treated_as_creds() {
        for u in [
            "https://bestapi.store/login",
            "https://bestapi.store/dashboard",
            "https://accounts.google.com/o/oauth2/auth",
        ] {
            let url = url::Url::parse(u).unwrap();
            assert!(parse_creds_navigation(&url).is_none(), "{u}");
        }
    }

    #[test]
    fn full_credentials_roundtrip() {
        let url = creds_url(
            r#"{"auth_token":"tok-abc","refresh_token":"ref-xyz","token_expires_at":"1800000000"}"#,
        );
        let creds = parse_creds_navigation(&url).unwrap().unwrap();
        assert_eq!(creds.auth_token, "tok-abc");
        assert_eq!(creds.refresh_token.as_deref(), Some("ref-xyz"));
        assert_eq!(creds.token_expires_at, Some(1_800_000_000));
    }

    #[test]
    fn millisecond_expiry_is_normalized_to_seconds() {
        // sub2api 前端存的是毫秒。不归一会把 2027 年的过期时间当成 57000 年，
        // 于是永远判为「没过期」，token 失效后一直不重登。
        let url = creds_url(r#"{"auth_token":"t","token_expires_at":"1800000000000"}"#);
        let creds = parse_creds_navigation(&url).unwrap().unwrap();
        assert_eq!(creds.token_expires_at, Some(1_800_000_000));
    }

    #[test]
    fn degraded_response_without_refresh_is_still_accepted() {
        // 服务端 GenerateTokenPair 失败时只返 access_token。这是「可用但不可续期」，
        // 必须收下 —— 判成失败会把用户推回反复登录，撞 /auth/login 的 20 次/分钟限流。
        let url = creds_url(r#"{"auth_token":"tok","refresh_token":null,"token_expires_at":null}"#);
        let creds = parse_creds_navigation(&url).unwrap().unwrap();
        assert_eq!(creds.auth_token, "tok");
        assert!(creds.refresh_token.is_none());
        assert!(creds.token_expires_at.is_none());
    }

    #[test]
    fn unparsable_expiry_does_not_discard_the_whole_credential() {
        let url = creds_url(r#"{"auth_token":"tok","token_expires_at":"not-a-number"}"#);
        let creds = parse_creds_navigation(&url).unwrap().unwrap();
        assert_eq!(creds.auth_token, "tok");
        assert!(creds.token_expires_at.is_none());
    }

    #[test]
    fn missing_access_token_is_a_visible_error() {
        // 静默当成「登录失败」会诱导用户反复登录；必须报出来。
        for payload in [r#"{"auth_token":null}"#, r#"{"auth_token":""}"#, "{}"] {
            let url = creds_url(payload);
            let err = parse_creds_navigation(&url).unwrap().unwrap_err();
            assert!(
                err.to_string().contains("access token"),
                "payload {payload}: {err}"
            );
        }
    }

    #[test]
    fn malformed_payloads_are_errors_not_panics() {
        let no_query = url::Url::parse(&format!("{CREDS_SCHEME}://ok")).unwrap();
        assert!(parse_creds_navigation(&no_query).unwrap().is_err());

        let bad_b64 = url::Url::parse(&format!("{CREDS_SCHEME}://ok?d=!!!not-base64!!!")).unwrap();
        assert!(parse_creds_navigation(&bad_b64).unwrap().is_err());
    }

    #[test]
    fn script_guards_on_origin_and_targets_sub2api_keys() {
        let s = login_script("https://bestapi.store", "");
        assert!(s.contains(r#"ALLOWED_ORIGIN = "https://bestapi.store""#));
        assert!(s.contains("window.top !== window.self"));
        assert!(s.contains("auth_token"));
        assert!(s.contains(CREDS_SCHEME));
    }

    #[test]
    fn creds_are_sent_by_top_level_navigation_not_an_iframe() {
        // **不能用 iframe** —— 实测 sub2api 全站带 `frame-src` 白名单
        // （challenges.cloudflare.com / *.stripe.com / checkout.airwallex.com），
        // `loongport-creds://` 不在其中，iframe 的 src 会被 CSP 直接阻断，凭据发不出去。
        // 顶层导航不受这条 CSP 管（无 `navigate-to` 指令），且 on_navigation 返回 false
        // 会拦下它 —— 页面不会真的走掉。
        //
        // 这条测试是防回归的：iframe 看起来「更干净」（不碰主文档），我自己就写过一版，
        // 是查了线上响应头才发现走不通。
        let s = login_script("https://bestapi.store", "");
        assert!(
            s.contains("window.location.href ="),
            "凭据回传必须走顶层导航"
        );
        assert!(
            !s.contains("createElement('iframe')"),
            "不能用 iframe —— 会被 sub2api 的 frame-src CSP 拦掉"
        );
    }

    #[test]
    fn connected_banner_is_self_contained_and_idempotent() {
        // 这条提示条是「窗口不再自动关闭」的配套 —— 没有它用户不知道该干什么。
        let js = CONNECTED_BANNER_JS;
        // 幂等：eval 可能被调多次（用户重新登录），不能叠出两条。
        assert!(js.contains("getElementById(ID)"), "必须先查再插，避免重复");
        // 得告诉用户两件事：这边好了、窗口可以自己关。
        assert!(js.contains("已连接"), "要说清已经连上了");
        assert!(js.contains("关掉此窗口"), "要告诉用户可以自己关");
        // 不改页面自身结构 —— 运营商页面长什么样我们不该假设。
        assert!(js.contains("position:fixed"), "浮层不该挤占页面布局");
        // 它是整段 JS 常量，不做插值 —— 若有人改成模板，这条会提醒他注意转义。
        assert!(!js.contains("{}"), "不该有插值占位符");
    }

    #[test]
    fn script_json_encodes_origin_so_quotes_cannot_break_out() {
        // origin 来自用户输入。带引号的输入若被直接插进字符串字面量，就是脚本注入。
        let s = login_script("https://evil\" + alert(1) + \"", "");
        assert!(!s.contains("\" + alert(1) + \""), "origin 没被转义: {s}");
    }

    #[test]
    fn prefill_dispatches_an_input_event_not_just_a_value_assignment() {
        // 这条钉住最容易漏的那一步：sub2api 登录页是 Vue，输入框绑 v-model="formData.email"。
        // 只设 el.value 的话 DOM 上看得见字、但 formData.email 仍是空的 —— 提交上去是空邮箱，
        // 而且表现是「明明填了却说邮箱必填」，非常难查。v-model 听的是 input 事件。
        let s = login_script("https://bestapi.store", "me@x.com");
        assert!(s.contains(r#"PREFILL = "me@x.com""#), "预填值该传进脚本");
        assert!(
            s.contains("new Event('input'"),
            "必须派 input 事件，否则 Vue 的 v-model 收不到: {s}"
        );
        assert!(s.contains("bubbles: true"), "事件必须冒泡才能被框架监听到");
    }

    #[test]
    fn prefill_never_touches_the_password_field() {
        // 预填只该碰登录标识那个框。选择器若写宽成 `input` 或带 type=password，
        // 就会把邮箱填进密码框 —— 用户看到一串明文，还得自己清掉。
        let s = login_script("https://bestapi.store", "me@x.com");
        assert!(!s.contains("type=password"), "选择器不该匹配密码框: {s}");
        assert!(s.contains("'#email'"), "sub2api 的框是 id=email");
        // new-api 那类用 username（实测 LoginRequest.Username，无 email 校验），也要覆盖。
        assert!(s.contains("'#username'"), "多运营商要能兼容 username 形式");
    }

    #[test]
    fn prefill_is_skipped_when_there_is_nothing_to_fill() {
        // 首次登录没有可预填的值。此时脚本仍要正常工作（凭据回传不受影响），
        // 只是不填 —— 而不是填一个空串把用户已输入的东西清掉。
        let s = login_script("https://bestapi.store", "");
        assert!(s.contains(r#"PREFILL = """#));
        assert!(
            s.contains("if (prefilled || !PREFILL) return;"),
            "空值要早退"
        );
        // 凭据回传那半不受预填影响。
        assert!(s.contains(CREDS_SCHEME));
    }

    #[test]
    fn prefill_json_encodes_the_identifier_so_quotes_cannot_break_out() {
        // 这个值来自服务端返回的账号信息 —— 同样不能直接插进脚本。
        let s = login_script("https://bestapi.store", "a\" + alert(1) + \"");
        assert!(!s.contains("\" + alert(1) + \""), "登录标识没被转义: {s}");
    }

    #[test]
    fn prefill_does_not_overwrite_what_the_user_already_typed() {
        // 用户可能正要换个账号登录。已经有值就别覆盖。
        let s = login_script("https://bestapi.store", "me@x.com");
        assert!(
            s.contains("if (el.value)"),
            "已有输入时必须让用户自己的输入胜出: {s}"
        );
    }

    #[test]
    fn login_url_points_at_login_not_register() {
        // register 在运营商关闭注册时是死页（只显示黄条），login 页脚有去注册的链接。
        assert_eq!(
            login_url("https://bestapi.store"),
            "https://bestapi.store/login"
        );
    }

    #[test]
    fn user_agent_is_shared_with_the_http_client() {
        // 会话绑定开启时 UA 不一致会 401 并撤销整个会话家族。这条钉住「两边用同一个常量」。
        assert!(!WEBVIEW_USER_AGENT.is_empty());
        assert!(WEBVIEW_USER_AGENT.contains("LoongPort"));
    }

    #[test]
    fn login_window_label_is_not_the_main_window() {
        // 两件事都靠这个不等式：
        //
        // 1. 与 `tauri.conf.json` 的 `app.windows` 里的 label 重名会在 setup 里 panic。
        // 2. **`lib.rs` 的全局 `CloseRequested` 回调用 `MAIN_WINDOW_LABEL` 当守卫** ——
        //    只有主窗口才走「最小化到托盘」。若登录窗的 label 变成了 `main`，它关闭时会被
        //    `prevent_close` 吃掉、隐藏后仍占 label，用户再点登录就卡死。
        //
        // 引用常量而不是写死 "main"：那样改常量时这条测试跟着走。
        assert_ne!(LOGIN_WINDOW_LABEL, crate::MAIN_WINDOW_LABEL);
    }
}
