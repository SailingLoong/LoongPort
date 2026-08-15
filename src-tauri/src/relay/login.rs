//! 登录 WebView：加载中转站真实登录页，登录成功后把 localStorage 里的凭据回传原生侧。
//!
//! ## 凭据怎么回来的
//!
//! Tauri 的 `eval` 不回传返回值，而给远端页面开 IPC 需要 `remote` capability（等于让中转站
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
//! （`LoongPort/src-tauri/src/relay/{title_channel.rs,login_script.js}`）。
//!
//! ## 凭据到手之后：窗口不得再持有续期能力
//!
//! 中转站的登录凭据是**一次性轮换**的 refresh lineage，只能有一个持有者 —— 本仓 DB。
//! 登录窗在回传凭据的那一刻起就必须把续期能力交出去，否则窗口里站点的自动续期
//! 会把 DB 里那把作废（详见 [`strip_refresh_keys_js`]）：
//!
//! - **sub2api**：eval [`strip_refresh_keys_js`] 删掉两个续期键。窗口继续活着
//!   （用户看面板、浏览器代拉走它的页面上下文），但只剩「只读」的登录态。
//! - **NewAPI**：登录 cookie 是 HttpOnly，JS 删不掉 ⇒ 没有卸能力的手段，只能
//!   凭据到手即 `destroy()`（见 `commands::relay` 两条登录路径的说明）。
//!
//! ## 落哪个页面：新站 `/register`，重登 `/login`
//!
//! 判据是这一行有没有 `login_identifier`（成功登录过才有）。见 [`login_url`]。
//!
//! ⚠️ **2026-08-04 改过一次**，原来是「一律 `/login`」，理由写的是
//! 「`/register` 在关闭注册时是死页」—— 那个理由**只对了一半**：那一页确实
//! 只剩一条黄条，但**页脚「已有账号？登录」在 `v-if/v-else` 之外**
//! （`RegisterView.vue` 的 `#footer` slot）⇒ 用户一点就走，困不住。
//! 而「刚开始大部分是新户」这条产品事实压过了那半个理由。

use serde::Serialize;

use crate::error::AppError;

/// 登录窗口的 label。
///
/// **不得与 `tauri.conf.json` 的 `app.windows` 里任何 label 重名** —— 两边都声明同一个
/// label 会在 setup hook 里 panic（`a webview with label "..." already exists`）。
pub const LOGIN_WINDOW_LABEL: &str = "loongport-login";

/// 凭据回传用的自定义 scheme。
///
/// 选一个绝不会被真实网站用到的值：注入脚本只在中转站 origin 下跑，但万一页面自己跳到某个
/// 我们认得的 scheme，就会被误当成凭据回传。
pub(crate) const CREDS_SCHEME: &str = "loongport-creds";

/// 凭据回传的 host 标记（与浏览器代拉共用 `loongport-creds` scheme，用 host 区分）。
pub(crate) const CREDS_CALLBACK_HOST: &str = "ok";

/// sub2api 存 access token 的 localStorage 键名。
///
/// ## 为什么提成常量（登录窗只读、充值窗要写）
///
/// [`login_script`] 读它判断「登录完了没」，而 [`crate::relay::purchase`] 要**写**它
/// 把登录态注入进充值页。两处各写一遍字面量的话，哪天 sub2api 改了键名就会变成
/// 「登录还好着、充值窗静默落到登录页」—— 那种不一致没有任何闸能发现。
///
/// 事实源：`upstream/sub2api/frontend/src/stores/auth.ts:11-14`。
pub const AUTH_TOKEN_KEY: &str = "auth_token";

/// 用户档案的键名（值是 `JSON.stringify(userData)` 的整段 JSON 字符串）。
///
/// ⚠️ **登录窗不读它，但充值窗必须写它** —— 站点的 `isAuthenticated` 是
/// `!!token.value && !!user.value`（auth.ts:110），且 `checkAuth()` 里
/// `if (savedToken && savedUser)` 缺这一项就**整块跳过**（连 token 的赋值都不执行）
/// ⇒ 只注入 token 的话窗口会落到登录页，而不是充值页。
pub const AUTH_USER_KEY: &str = "auth_user";

/// refresh token 的键名。
///
/// ⚠️ **登录窗读完要删、充值窗有意不写** —— 两边是同一条不变式的两个方向：
/// refresh lineage 的唯一持有者是本仓 DB，任何窗口都不该持有能触发轮换的副本。
///
/// - sub2api 的 refresh token 是**一次性轮换**的：站点用掉之后本仓 DB 里那份
///   立刻失效，下次本仓自己续期时服务端认不出它、返回
///   `REFRESH_TOKEN_INVALID` ⇒ **用户被迫重新登录整个中转站**。见
///   [`strip_refresh_keys_js`] 与 [`crate::relay::purchase`] 的模块文档。
/// - 充值窗不写它的理由同上（写进去站点一定用掉）。
pub const REFRESH_TOKEN_KEY: &str = "refresh_token";

/// 过期时间的键名。**站点存的是毫秒时间戳字符串**（见 [`decode_creds`] 归一成秒）。
///
/// ⚠️ 与 [`REFRESH_TOKEN_KEY`] 同理：充值窗有意不写、登录窗读完即删（它是站点
/// 起续期定时器的依据，删掉等于连定时器都不再排）。见 [`strip_refresh_keys_js`]。
pub const TOKEN_EXPIRES_AT_KEY: &str = "token_expires_at";

/// 从登录页取回的凭据。
#[derive(Debug, Clone, Serialize)]
pub struct Credentials {
    pub auth_token: String,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<i64>,
    pub user_agent: Option<String>,
    /// Cloudflare 放行 cookie（`cf_clearance`），没开挑战的站是 `None`。
    ///
    /// 由原生 API 从 WebView 读出 —— 它是 HttpOnly，注入脚本读不到，
    /// 所以不走 `trySend()` 那条 JSON 回传，见 `commands::relay` 的取用点。
    pub cf_clearance: Option<String>,
}

/// 凭据到手后浮在登录页顶部的提示条。
///
/// 存在的理由：**窗口不再自动关闭**（见 `commands::relay::do_login` 里那段说明），所以
/// 必须告诉用户「这边已经好了，你可以继续用这个页面，也可以关掉它」—— 否则他不知道该干什么，
/// 只会盯着一个看起来没反应的窗口。
///
/// 写成一整段自执行 JS 常量而不是模板：它没有任何需要插值的东西，做成 `format!` 只会引入
/// 转义风险。
///
/// 用 `position: fixed` + 高 `z-index`，不改页面自身的任何 DOM 结构 —— 中转站的页面长什么样
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

/// 生成「卸掉登录窗续期能力」的脚本：删掉 localStorage 里的 `refresh_token` 与
/// `token_expires_at` 两个键。
///
/// ## 为什么必须删（登录窗是 refresh lineage 的第二个持有者）
///
/// 登录成功那一刻，同一把 sub2api refresh token 同时存在于两处：本仓 DB（原生侧
/// `usable_relay` 靠它续期）和这个窗口的 localStorage。它是**一次性轮换**的 ——
/// 站点前端到点（access token 过期前 2 分钟）会起定时器用它换新对，而换出的新
/// refresh token 只写进窗口的 localStorage，关窗即失 ⇒ DB 里那把当场作废 ⇒
/// 用户被迫重登整个中转站。窗口开得越久（或站点把 access TTL 配得越短），死得越快。
///
/// 删掉这两个键后，窗口**没有任何路径能再触发轮换**（有上游源码依据，两条路都读
/// localStorage，不是内存缓存）：
///
/// - 主动定时器：`performTokenRefresh` → `refreshAuthTokens` → `readAuthSnapshot`
///   （`tokenRefresh.ts:52`）`getItem('refresh_token')` 为 null 直接 throw，被
///   `performTokenRefresh` 的 catch 吞掉、明确不清登录态；
/// - 401 拦截器：`client.ts:166` 同样 `getItem`，拿不到就不续期。
///
/// 这是充值窗「只注入两个键」决定的镜像（那边是不写、这边是删掉）—— 两边共同的
/// 不变式：**窗口永远不持有续期能力，refresh lineage 的唯一持有者是本仓 DB**。
///
/// 浏览器代拉不受影响：代拉 fetch 重放的是我们自己的头（Bearer 来自 DB），不经过
/// 站点自己的 axios 拦截器。代价与充值窗同款：窗口里的页面在 access token 过期后
/// 会被 401 拦截器清掉登录态、硬跳 `/login` —— 对一个登录窗来说页面寿命到那时
/// 早已用完，可接受。
///
/// 用 `format!` 而不是整段字面量：键名必须复用 [`REFRESH_TOKEN_KEY`] /
/// [`TOKEN_EXPIRES_AT_KEY`] 常量 —— 站点改键名时这里跟着走，不会出现
/// 「充值窗改了、登录窗没改」的静默分叉。
pub fn strip_refresh_keys_js() -> String {
    format!(
        r#"(function () {{
  try {{
    localStorage.removeItem({REFRESH_TOKEN_KEY:?});
    localStorage.removeItem({TOKEN_EXPIRES_AT_KEY:?});
  }} catch (e) {{ /* 私有模式等极端情况下的 localStorage 异常不值得惊动任何人 */ }}
}})();"#
    )
}

/// 生成注入脚本。
///
/// `site_origin` 用于 origin 守卫：脚本只在中转站自己的页面上生效，跳到第三方（OAuth 授权页
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
///
/// `promo_code` 是注册优惠码（`None` = 这个站没有，见 [`super::promo`]）。
/// 与 `aff_code` 是**两个不同的服务端字段**，可以同时带。
pub fn login_script(
    site_origin: &str,
    login_identifier: &str,
    aff_code: Option<&str>,
    promo_code: Option<&str>,
) -> String {
    // JSON 编码 origin 而不是直接插进单引号里：origin 来自用户输入，含引号就会破坏脚本语法。
    let origin_literal = serde_json::to_string(site_origin).unwrap_or_else(|_| "\"\"".to_string());
    // 同理 JSON 编码：这个值来自服务端返回的账号信息，含引号就会破坏脚本语法。
    let identifier_literal =
        serde_json::to_string(login_identifier).unwrap_or_else(|_| "\"\"".to_string());

    // 邀请码由**调用方**决定（它按「远端 > 缓存 > 内置」三层解析，见
    // `super::remote_config::resolve_aff_code`）。本模块不自己查表 ——
    // 那样远端那层永远进不来，而且会让 login 依赖 aff 的实现细节。
    //
    // `None` ⇒ **整段不生成**：绝大多数站走这条路（包括维护者自己的站），
    // 那时脚本里连提都不该提这件事。
    let aff_snippet = aff_code.map(aff_seed_snippet).unwrap_or_default();

    // 优惠码同理由调用方给（本模块不查表）。
    //
    // ⚠️ **`None` 时必须发一个空实现，不能像 `aff_snippet` 那样整段留空**
    // （review 2026-08-04 抓出的 P0）：`tryPrefillPromo()` 在主脚本里被
    // **无条件调用两次**（立即一次 + 轮询里每轮一次），而它的定义在这段
    // snippet 内 ⇒ 留空就是 `ReferenceError: tryPrefillPromo is not defined`。
    //
    // 后果比「优惠码没填上」严重得多，而且**打中的是常见情况**（表里只有
    // 一个站，其余全部走 `None`）：轮询回调里那个 throw 让
    // `if (sent || polls > 600) clearInterval(timer)` 永远到不了 ⇒ 定时器
    // **永不清除**。凭据回传只是因为 `trySend()` 排在它前面才侥幸没坏 ——
    // 那是顺序上的运气，不是设计。
    //
    // `aff_snippet` 没这个问题是因为它**自包含**（一整段 `try { … }`，
    // 主脚本里没有对它的调用点）。两者形状不同，别照着 aff 那行写。
    let promo_snippet = promo_code
        .map(promo_prefill_snippet)
        .unwrap_or_else(|| "  function tryPrefillPromo() {}\n".to_string());

    // 注册页顶部的横幅。**无条件生成**（与 `aff_snippet` 那种有条件的不同）——
    // 它自己判「当前在不在 /register」，重登落 `/login` 时那段就是个空转的定时器。
    // 判据放在 JS 里而不是这里，是因为 SPA 路由切换不重跑脚本：用户可能从
    // `/login` 点「去注册」过去，那时也该有横幅。
    let register_hint_snippet = register_hint_banner_snippet();

    format!(
        r#"(function () {{
  'use strict';

  // 只在顶层 frame 跑：同源 iframe 会让脚本多执行一份、重复回传。
  if (window.top !== window.self) return;

  var ALLOWED_ORIGIN = {origin_literal};
  // ⚠️ **这条 early-return 是白屏的一个真实候选**（2026-08-04 加日志时识别出来）：
  // 站点若把我们重定向到另一个 origin（自定义域 → 主域、http → https、
  // 或前置了一层 SSO），脚本**整段不执行**且不留任何痕迹 ⇒ 凭据永远不回传，
  // 用户看到的就是一个不动的窗口。`console.warn` 让它至少在 WebView 控制台可见
  // （Windows 上 `--remote-debugging-port` 或 devtools 能看到）。
  if (window.location.origin !== ALLOWED_ORIGIN) {{
    console.warn('[LoongPort] 脚本未启用：当前 origin', window.location.origin,
                 '≠ 期望', ALLOWED_ORIGIN, '—— 凭据不会回传');
    return;
  }}

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

{promo_snippet}
{aff_snippet}
{register_hint_snippet}
  // sub2api 前端的凭据键名。**从 Rust 常量插进来** —— 充值窗要写同样这几个键
  // （见 AUTH_TOKEN_KEY 那组的文档），两处各写一遍字面量迟早会漂。
  var K_TOKEN = '{AUTH_TOKEN_KEY}';
  var K_REFRESH = '{REFRESH_TOKEN_KEY}';
  var K_EXPIRES = '{TOKEN_EXPIRES_AT_KEY}';

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
      token_expires_at: window.localStorage.getItem(K_EXPIRES),
      user_agent: navigator.userAgent,
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
    window.location.href = '{CREDS_SCHEME}://{CREDS_CALLBACK_HOST}?d=' + b64url(payload);
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
    // ⚠️ 优惠码那个框**必须一直轮询到登录成功**，不能像 tryPrefill 那样填一次就收手：
    // 用户可能先落在登录页（那儿没有这个框）、点「去注册」才到注册页。
    // 站内路由跳转不会重跑 initialization_script，所以只有轮询能等到它出现。
    tryPrefillPromo();
    if (sent || polls > 600) clearInterval(timer);
  }}, 500);
  trySend();
  tryPrefill();
  tryPrefillPromo();
}})();
"#
    )
}

/// 生成「把注册优惠码填进注册表单」的那一小段脚本。
///
/// ## 为什么是 DOM 预填，而 aff 是写 localStorage（两者有意不同）
///
/// 差异不在我们的偏好，在**站点给的接口不同**：
///
/// | | aff 码 | 优惠码 |
/// |---|---|---|
/// | URL 参数 | `?aff=` / `?aff_code=` | `?promo=`（`RegisterView.vue:501`） |
/// | localStorage 回落 | **有** —— `affiliate_referral_code` 键 | **没有** |
/// | 路由变化时重读 | **有** `watch`（`RegisterView.vue:518`） | **没有**，只在 `onMounted` 读一次 |
/// | ⇒ 我们只能 | 写那个键（跨站内跳转仍有效） | **填 DOM** |
///
/// 优惠码既没有持久化回落、也没有路由 watcher ⇒ 那个 query 只在
/// **`RegisterView` 挂载那一瞬间**被读一次。所以就算我们给 URL 挂上 `?promo=`：
/// 落 `/login` 再跳过去时 `RegisterView` 是新挂载的、但 URL 已经没有 query 了
/// ⇒ 读到空。只剩 DOM 预填这条路。
///
/// ## 三条克制
///
/// 1. **用户已经自己填了就不覆盖** —— 他可能拿到了比我们这个更好的码。
/// 2. **必须派 `input` 事件**（与登录标识预填同一条理由）：只设 `.value` 的话
///    Vue 的 `formData.promo_code` 仍是空的，提交上去等于没填。
///    而且站点的实时校验（`validatePromoCodeDebounced`）也挂在 `input` 上 ——
///    不派事件那个「+N 赠额」的绿条不会出现，用户不知道码生效了。
/// 3. **整段包在 try 里，失败什么都不做**：与 aff 同理，优惠码不该打断登录。
///
/// ## ⚠️ 为什么每轮都试，但**按元素身份**记「填过了」
///
/// [`login_script`] 里它挂在轮询上、**每轮都跑**（而 `tryPrefill` 填一次就置
/// `prefilled = true`）。因为那个框只存在于 `/register`，而用户可能先落在
/// `/login`、点页脚「去注册」才过去 —— 站内路由跳转**不重跑**
/// `initialization_script`，填一次就收手会让这种路径永远填不上。
///
/// 但收手的判据**不能用「框里有没有值」**（codex review 2026-08-04 抓出）：
/// 那样用户**清不掉这个码** —— 他删空，500ms 后我们又填回去，
/// 而站点的 `handleInput` 明确允许空值（清掉校验态，`RegisterView.vue:598`）。
/// 换个非空码可以，唯独「不想要任何码」做不到，那是把提供便利变成强加。
///
/// 所以按**元素身份**记（`WeakSet`）：每个 `#promo_code` 元素只填一次，
/// 之后用户怎么改都不再碰它。而 `/login` ↔ `/register` 之间来回跳会让 Vue
/// **重新挂载**一个新元素（两者是独立的路由组件，`router/index.ts:43`）
/// ⇒ 回到注册页仍然填得上。
///
/// `WeakSet` 而不是数组：元素被卸载后不该留一份强引用拖着它不回收。
fn promo_prefill_snippet(code: &str) -> String {
    // 与 aff 同理：码来自人手录的编译期表，JSON 编码防注入。
    let code_literal = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"
  // 预填注册优惠码（用户得赠额）。**只能填 DOM** —— 站点的优惠码没有 localStorage
  // 回落（只读 `?promo=` query），而我们跨站内路由，query 会丢。见 Rust 侧文档。
  var PROMO = {code_literal};
  // 记「哪些框已经填过」，按**元素身份**而不是按「有没有值」——
  // 后者会让用户清不掉这个码（删空后 500ms 又被填回）。
  var promoFilled = typeof WeakSet === 'function' ? new WeakSet() : null;

  function tryPrefillPromo() {{
    if (!PROMO) return;
    try {{
      // 只认 sub2api 那个精确 id（`RegisterView.vue:169`）。**不做泛化选择器** ——
      // 猜错了会把码填进别的框（邮箱 / 邀请码），比不填糟得多。
      var el = document.querySelector('#promo_code');
      // 框不存在就下轮再看。两种情况都走这条：还没渲染出来（异步），或者
      // **这个站关掉了优惠码功能**（那个框包在 `v-if="promoCodeEnabled"` 里，
      // `RegisterView.vue:159`）。后者会让我们每 500ms 白查一次，最多 5 分钟 ——
      // ⚠️ **那是可接受的，别去「优化」它**：要提前知道就得读站点 settings，
      // 为一次失败的 querySelector 引入一个网络依赖不值得。
      if (!el) return;
      // 这个框碰过就永不再碰 —— 之后用户清空、改成别的码，都是他的决定。
      // 「碰过」按**元素身份**记（不是按「有没有值」，那会让用户清不掉这个码）。
      if (promoFilled && promoFilled.has(el)) return;
      // 无论下面填不填，这个元素都算处理过了。**必须记在两条出路之前** ——
      // 只在填成功那条记的话，「用户已有输入」那条会每轮重新走一遍，
      // 于是他把框清空的那一刻就被我们填上了（正是要避免的那个 bug）。
      if (promoFilled) promoFilled.add(el);
      // 用户已经自己输了东西就别覆盖 —— 他可能拿到了比我们这个更好的码。
      // ⚠️ WeakSet 不可用的老引擎走到这里时 promoFilled 是 null ⇒ 记不住 ⇒
      // 每轮重新判这一行。那是**有意的降级**：退回「有值就不填」的旧行为，
      // 用户清不掉码，但至少不会覆盖他的输入。
      if (el.value) return;
      el.value = PROMO;
      // 必须派事件：v-model 要靠它同步 formData，站点的实时校验也挂在 input 上
      // （不派的话那条「+N 赠额」的绿色确认条不会出现）。
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      el.dispatchEvent(new Event('change', {{ bubbles: true }}));
    }} catch (e) {{
      // 优惠码是附加好处，绝不为它打断登录/注册。
    }}
  }}
"#
    )
}

/// 注册页顶部那条「这是注册页，有账号请去登录」的横幅。
///
/// ## 为什么需要它（用户提的）
///
/// 新站落 `/register`（见 [`login_url`]）。sub2api 的注册页**确实有**一个「已有账号？
/// 去登录」的链接，但它在 `<template #footer>` 里 —— 用户得滚到页底才看得见。
/// 于是已有账号的人在注册页上填一遍，被告知邮箱已注册，才发现走错了页。
///
/// ## 文案**复用页面自己的**，不自带 i18n
///
/// 横幅里那两句话直接取页脚那个链接及其前面那句提示的文字
/// （`RegisterView.vue` 的 `t('auth.alreadyHaveAccount')` + `t('auth.signIn')`）——
/// 那已经是**站点当前语言**的正确说法。
///
/// 自带一份四语文案是错的两次：Rust 侧没有 i18n 机制（要为它引一套），
/// 而且 LoongPort 的界面语言与站点的语言可能不同 —— 站点是英文界面时，
/// 横幅冒出两句中文比没有横幅更糟。
///
/// 取不到那个链接（站点改版 / 关闭了注册入口）⇒ **整段不显示**，
/// 而不是回落到硬编码文案：宁可没有这个提示，也不要显示一句可能是错的话。
///
/// ## 点击走站内路由，不整页跳
///
/// 横幅的按钮**模拟点击页脚那个 `router-link`**，而不是 `location.href = '/login'`。
/// 后者是整页刷新 ⇒ 重新执行注入脚本、重新读 localStorage，而更要紧的是
/// **`?aff=` 那套关系在整页跳转里会丢**（见 [`aff_seed_snippet`]：我们靠 localStorage
/// 种它，而整页刷新后站点的 `resolveAffiliateReferralCode` 会重跑一遍）。
/// 点现成的链接让 Vue Router 处理，与用户自己点它完全等价。
///
/// ## 只在 `/register` 显示，路由切走就撤掉
///
/// 脚本注一次、SPA 路由切换**不重跑**（这是本模块反复踩到的事实，见
/// [`promo_prefill_snippet`] 的文档）。所以横幅自己盯 `location.pathname`：
/// 不在 `/register` 就移除。否则用户点了「去登录」，横幅会跟着留在登录页上，
/// 说着一句已经不成立的话。
fn register_hint_banner_snippet() -> String {
    // ⚠️ `r##` 而不是 `r#`：下面的选择器里有 `$="#/login"`，
    // 那个 `"#` 会提前终止 `r#"…"#`（编译器报的是「unknown prefix `login`」，
    // 看着完全不像引号问题）。
    r##"
  // 注册页顶部的横幅：文案取自页面自己的「已有账号？去登录」，见 Rust 侧
  // register_hint_banner_snippet 的文档（为什么不自带 i18n、为什么点现成的链接）。
  try {
    var BANNER_ID = 'loongport-register-hint';

    function findSignInLink() {
      var links = document.querySelectorAll('a[href="/login"], a[href$="#/login"]');
      for (var i = 0; i < links.length; i++) {
        // 可见且有文字的那个才是页脚那条（隐藏的、空的都跳过）。
        if (links[i].offsetParent !== null && links[i].textContent.trim()) return links[i];
      }
      return null;
    }

    // 是我们设过 body 的 paddingTop 吗。撤横幅时要还原它，而**只还原自己设的那次** ——
    // 站点自己可能也用这个属性（横幅在时我们只在它为空串时才设，见 syncBanner）。
    var paddedByUs = false;

    function removeBanner() {
      var old = document.getElementById(BANNER_ID);
      if (old) old.remove();
      // ⚠️ **必须还原**（review 抓出）：登录窗在拿到凭据后**有意不关**
      // （见 commands/relay.rs 那段「不关窗」的说明：dashboard 上有余额与充值入口，
      // 用户还要接着用）。不还原的话，他会带着一条 40px 的空白条浏览登录页、
      // dashboard、充值页 —— 而那条横幅早就不在了。
      if (paddedByUs) {
        document.body.style.paddingTop = '';
        paddedByUs = false;
      }
    }

    function syncBanner() {
      // 只在注册页显示。`indexOf` 而不是 `===`：站点可能跑在 hash 路由下
      // （`/#/register`），那时 pathname 是 `/`、路径在 hash 里。
      var onRegister = window.location.pathname.indexOf('/register') !== -1
        || window.location.hash.indexOf('/register') !== -1;
      if (!onRegister) { removeBanner(); return; }
      if (document.getElementById(BANNER_ID)) return;

      var link = findSignInLink();
      // 取不到就不显示 —— 见文档：宁可没提示也不要一句可能是错的话。
      if (!link) return;

      var prompt = '';
      // 链接前面那句提示（`已有账号？`）在同一个父节点的文本里。
      var parentText = link.parentNode ? link.parentNode.textContent : '';
      if (parentText) prompt = parentText.replace(link.textContent, '').trim();

      var bar = document.createElement('div');
      bar.id = BANNER_ID;
      bar.setAttribute('role', 'status');
      // 内联样式：站点的 CSS 类名不稳定（改版就失效），而这条横幅必须一直看得见。
      bar.style.cssText = [
        'position:fixed', 'top:0', 'left:0', 'right:0', 'z-index:2147483647',
        'display:flex', 'align-items:center', 'justify-content:center', 'gap:10px',
        'padding:10px 16px', 'background:#dc2626', 'color:#fff',
        'font:500 14px/1.5 system-ui,-apple-system,sans-serif',
        'box-shadow:0 1px 3px rgba(0,0,0,.2)'
      ].join(';');

      var text = document.createElement('span');
      text.textContent = prompt || link.textContent;
      bar.appendChild(text);

      var btn = document.createElement('button');
      btn.type = 'button';
      btn.textContent = link.textContent.trim();
      btn.style.cssText = [
        'padding:4px 12px', 'border:1px solid rgba(255,255,255,.6)', 'border-radius:6px',
        'background:rgba(255,255,255,.15)', 'color:#fff', 'cursor:pointer',
        'font:inherit'
      ].join(';');
      btn.addEventListener('click', function () {
        // **点现成的链接**，让 Vue Router 处理 —— 见文档：整页跳会丢 aff 关系。
        // 每次点都重新查一遍：SPA 里那个节点可能已经被替换过。
        var fresh = findSignInLink();
        if (fresh) fresh.click();
      });
      bar.appendChild(btn);

      document.body.appendChild(bar);
      // 别盖住页面顶部的内容。只在站点自己没设过这个属性时才动它，
      // 并记下「是我们设的」—— `removeBanner` 靠那个标志决定要不要还原。
      if (document.body.style.paddingTop === '') {
        document.body.style.paddingTop = bar.offsetHeight + 'px';
        paddedByUs = true;
      }
    }

    // 首屏 + 之后每 500ms 同步一次。
    //
    // **不用 MutationObserver / popstate**：这是个 Vue SPA，路由切换既不发
    // `popstate`（`router.push` 走的是 `history.pushState`）也不一定改动我们
    // 盯得住的节点。轮询是这个脚本里已有的模式（`trySend` / `tryPrefill` 同样如此），
    // 500ms × 一个 querySelector 的开销可以忽略。
    //
    // ⚠️ **必须设上限**（review 抓出我写错的一个前提）。原来这里写的是「登录窗的
    // 生命周期就是用户这一次登录，窗口关掉定时器随之消失」—— **那不成立**：
    // 拿到凭据后窗口**有意不关**（见 commands/relay.rs 那段「不关窗」），
    // 用户会在里面接着看 dashboard、充值页，想看多久看多久。于是这个定时器会
    // 一直轮询下去。
    //
    // 上限用 5 分钟（600 × 500ms），与主脚本那个轮询同一个数量级：横幅只在
    // 注册/登录这一小段里有意义，用户走到 dashboard 之后它永远不会再显示。
    // 到点前若已经离开注册页，撤掉横幅并停表。
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', syncBanner);
    } else {
      syncBanner();
    }
    var polls = 0;
    var timer = setInterval(function () {
      polls++;
      if (polls > 600) {
        clearInterval(timer);
        removeBanner();
        return;
      }
      syncBanner();
    }, 500);
  } catch (e) {
    // 横幅是个提示，不是登录必需的一步。它坏了绝不能影响凭据回传。
    console.warn('[LoongPort] 注册页横幅未能显示:', e);
  }
"##
    .to_string()
}

/// 生成「把邀请码种进 localStorage」的那一小段脚本。
///
/// ## 三条克制（每条都有理由，别"改进"它们）
///
/// 1. **只在键不存在或已过期时写** —— 用户可能是**别人邀请来的**（他自己带着
///    `?aff=` 进过这个站，站点已经存了那个码）。覆盖它等于抢别人的邀请关系。
///    判据用站点自己的语义（`expiresAt <= Date.now()` 视为没有），不自己发明。
/// 2. **`expiresAt` 用与站点相同的 30 天 TTL**，不写成永久 —— 写成永久会让一个
///    早已过期的码在站点看来仍然有效，那是在给它塞脏数据。
/// 3. **整段包在 try 里，失败什么都不做**：邀请码是**我们的收益**，不是用户要的功能。
///    为它打断登录流程是本末倒置。
///
/// 与凭据回传那半**完全解耦**：它读、这段写，互不依赖 —— 邀请码这段整个删掉，
/// 登录照样工作。
fn aff_seed_snippet(code: &str) -> String {
    // 码来自我们自己的编译期常量表，但仍然 JSON 编码 —— 那张表是人手录的，
    // 哪天录进一个带引号的值不该变成脚本注入。
    let code_literal = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"
  // 种下注册邀请码（我们的返利关系）。站点的 `resolveAffiliateReferralCode` 在 URL
  // 没带 `?aff=` 时会回落读这个键 —— 而我们打开的 URL（新站 /register、
  // 重登 /login）两条都不带 query，站内跳转也不会带上，所以只能走 localStorage。
  // 详见 Rust 侧 AFFILIATE_REFERRAL_CODE_KEY 的文档。
  try {{
    var AFF_KEY = '{AFFILIATE_REFERRAL_CODE_KEY}';
    var AFF_TTL_MS = {AFFILIATE_REFERRAL_TTL_MS};
    var existing = null;
    try {{ existing = JSON.parse(window.localStorage.getItem(AFF_KEY) || 'null'); }} catch (e) {{}}
    // **已有一个还没过期的码就不动它** —— 用户可能是别人邀请来的，覆盖等于抢关系。
    var stillValid = existing && existing.code && Number(existing.expiresAt) > Date.now();
    if (!stillValid) {{
      window.localStorage.setItem(AFF_KEY, JSON.stringify({{
        code: {code_literal},
        expiresAt: Date.now() + AFF_TTL_MS
      }}));
    }}
  }} catch (e) {{
    // 存储不可用。邀请码是我们的收益不是用户要的功能，绝不为它打断登录。
  }}
"#
    )
}

/// 注册邀请码的 localStorage 键名（站点的 `utils/oauthAffiliate.ts:2`）。
///
/// ⚠️ **值不是裸字符串**，是 `{"code":"...","expiresAt":<毫秒时间戳>}` 的 JSON。
/// 站点的 `resolveAffiliateReferralCode` 在 URL 没带 `?aff=` 时回落读它
/// （`loadAffiliateReferralCode`，过期即视为没有）。
///
/// ## 为什么走 localStorage 而不是给 URL 挂 `?aff=`
///
/// 挂 URL **看起来**更简单（`RegisterView.vue:466` 确实读 `route.query.aff`），
/// 但**两条路径下它都拿不到**：
///
/// - **新站落 `/register`**（见 [`login_url`]）：我们打开的那个 URL 本身就不带
///   query —— 要挂就得在 `login_url` 里拼，而那会把「邀请码」这件事漏进一个
///   只该管「落哪个页面」的函数。
/// - **重登落 `/login`**：`LoginView.vue:191` 那个「去注册」链接是
///   **`to="/register"` 裸路径、不带 query** ⇒ 用户一点 `?aff=` 就丢了。
///
/// 写进 localStorage 两条路径都成立，且不受站内路由跳转影响。
///
/// ⚠️ 2026-08-04 前这里写的是「我们加载的是 `/login` 而不是 `/register`
/// （后者是死页）」—— 那个前提已经不成立了（新站现在就落 `/register`），
/// 但结论不变，只是理由换成了上面第一条。
const AFFILIATE_REFERRAL_CODE_KEY: &str = "affiliate_referral_code";

/// 站点给邀请码的 TTL（`oauthAffiliate.ts:3` 的 `AFFILIATE_REFERRAL_TTL_MS`）。
///
/// 与站点写同一个值，而不是写成永久 —— 永久会让一个早已过期的码在站点看来仍有效。
const AFFILIATE_REFERRAL_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 登录窗要加载哪个页面。
///
/// `login_identifier` 空 = **这一行还没成功登录过**（新加的站）⇒ 落在 `/register`；
/// 非空 = 重登 ⇒ 落在 `/login`（那个标识就是给它预填用的）。
///
/// ## 为什么新站落注册页（2026-08-04 维护者拍板）
///
/// 「刚开始大部分都是新户」—— 新加一个站的人通常还没有那个站的账号，
/// 落在登录页要多点一次「去注册」。而已经登录过的行是重登，落注册页是明确的退步。
///
/// 判据用 `login_identifier` 而不是新开一个「登录过没有」的列：那个值**正是**
/// 「成功登录后才写进去的东西」（`commands/relay.rs` 里 `op.login_identifier =
/// account.email`），空与非空已经精确表达了这件事，再加一列是同一事实存两份。
///
/// ## ⚠️ 为什么不看 `registration_enabled`
///
/// 站点关闭注册时 `/register` 只把表单换成一条黄条（`RegisterView.vue:16-27`），
/// **但页脚那句「已有账号？登录」在 `v-if/v-else` 之外**（`RegisterView.vue:306-317`
/// 的 `#footer` slot）⇒ 用户照样一点就到登录页，不会被困住。
///
/// 所以不必为此持久化 `registration_enabled`（现在它只在 probe 时返回给前端、不入库）。
/// 少一个要与服务端保持同步的本地状态 —— 那个值会过期，而它过期时的后果
/// 正好是我们本来就能接受的那种（多点一次链接）。
pub fn login_url(site_origin: &str, login_identifier: &str) -> String {
    if login_identifier.is_empty() {
        format!("{site_origin}/register")
    } else {
        format!("{site_origin}/login")
    }
}

/// Cloudflare 托管挑战通过后种下的放行 cookie 名。
const CF_CLEARANCE_COOKIE_NAME: &str = "cf_clearance";

/// 从 WebView 的 cookie 列表里挑出 `cf_clearance`。
///
/// 它是 **HttpOnly**，注入脚本读不到 ⇒ 必须走原生 `cookies_for_url`，
/// 不能像 access token 那样从 localStorage 回传。
///
/// 没有该 cookie 是**正常情况**（绝大多数站没开托管挑战），返回 `None`。
pub fn extract_cf_clearance(cookies: &[tauri::webview::Cookie<'_>]) -> Option<String> {
    cookies
        .iter()
        .find(|cookie| {
            cookie.name() == CF_CLEARANCE_COOKIE_NAME && !cookie.value().trim().is_empty()
        })
        .map(|cookie| cookie.value().to_string())
}

/// 判断一次导航是不是凭据回传，是则解出凭据。
///
/// 返回 `None` 表示这是普通导航（放行）；返回 `Some` 表示这是回传（调用方应拦下）。
pub fn parse_creds_navigation(url: &url::Url) -> Option<Result<Credentials, AppError>> {
    if url.scheme() != CREDS_SCHEME || url.host_str() != Some(CREDS_CALLBACK_HOST) {
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
        user_agent: Option<String>,
    }
    let raw: Raw = serde_json::from_str(&json)
        .map_err(|e| AppError::Config(format!("凭据回传的格式不对: {e}")))?;

    let auth_token = raw
        .auth_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Config("登录页没有给出 access token".into()))?;

    Ok(Credentials {
        auth_token,
        // 由原生 cookie 读取补上（HttpOnly，脚本回传里没有它）。
        cf_clearance: None,
        refresh_token: raw.refresh_token.filter(|t| !t.is_empty()),
        // sub2api 存的是毫秒时间戳字符串。解不出来不算错 —— 那就是「可用但不知何时过期」，
        // 与「没登录」是两件事，不该因此把整份凭据扔掉。
        token_expires_at: raw
            .token_expires_at
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(|ms| if ms > 100_000_000_000 { ms / 1000 } else { ms }),
        user_agent: raw.user_agent.filter(|s| !s.is_empty()),
    })
}

pub(crate) fn decode_b64url(s: &str) -> Option<String> {
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
    #[test]
    fn cf_clearance_is_extracted_only_when_present_and_non_empty() {
        let mut valid = tauri::webview::Cookie::new("cf_clearance", "pass-token");
        assert_eq!(
            extract_cf_clearance(std::slice::from_ref(&valid)),
            Some("pass-token".to_string())
        );

        // 空值等于没有：CF 没种下有效 cookie 时不该发一个空的出去。
        valid.set_value("   ");
        assert_eq!(extract_cf_clearance(std::slice::from_ref(&valid)), None);

        // 没开挑战的站压根没有这个 cookie —— 正常情况，不是错误。
        let others = [
            tauri::webview::Cookie::new("session", "s"),
            tauri::webview::Cookie::new("other", "o"),
        ];
        assert_eq!(extract_cf_clearance(&others), None);
    }

    /// 卸续期能力的脚本必须真的引用那两个键常量 —— 它靠 `format!` 接线，
    /// 哪天有人把插值改回字面量或删掉一个键，这个闸会让它当场红。
    #[test]
    fn strip_refresh_keys_js_targets_both_rotation_keys() {
        let script = strip_refresh_keys_js();
        assert!(
            script.contains(&format!("removeItem(\"{REFRESH_TOKEN_KEY}\")")),
            "缺少对 {REFRESH_TOKEN_KEY} 的删除：{script}"
        );
        assert!(
            script.contains(&format!("removeItem(\"{TOKEN_EXPIRES_AT_KEY}\")")),
            "缺少对 {TOKEN_EXPIRES_AT_KEY} 的删除：{script}"
        );
        // 不能误删登录态本体：页面在 access token 有效期内还得是已登录视图。
        assert!(!script.contains(AUTH_TOKEN_KEY));
        assert!(!script.contains(AUTH_USER_KEY));
    }

    use super::*;

    /// 不关心优惠码那一维的用例走这个（等价于「这个站没有优惠码」）。
    ///
    /// 有意保留 `aff_code` 那个参数：已有一批用例正是在测它的有无。
    fn script(site_origin: &str, login_identifier: &str, aff_code: Option<&str>) -> String {
        login_script(site_origin, login_identifier, aff_code, None)
    }

    fn callback_url(host: &str, payload: &str) -> url::Url {
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
        url::Url::parse(&format!("{CREDS_SCHEME}://{host}?d={bin}")).unwrap()
    }

    fn creds_url(payload: &str) -> url::Url {
        callback_url(CREDS_CALLBACK_HOST, payload)
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
        let s = script("https://bestapi.store", "", None);
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
        let s = script("https://bestapi.store", "", None);
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
        // 不改页面自身结构 —— 中转站页面长什么样我们不该假设。
        assert!(js.contains("position:fixed"), "浮层不该挤占页面布局");
        // 它是整段 JS 常量，不做插值 —— 若有人改成模板，这条会提醒他注意转义。
        assert!(!js.contains("{}"), "不该有插值占位符");
    }

    #[test]
    fn script_json_encodes_origin_so_quotes_cannot_break_out() {
        // origin 来自用户输入。带引号的输入若被直接插进字符串字面量，就是脚本注入。
        let s = script("https://evil\" + alert(1) + \"", "", None);
        assert!(!s.contains("\" + alert(1) + \""), "origin 没被转义: {s}");
    }

    #[test]
    fn prefill_dispatches_an_input_event_not_just_a_value_assignment() {
        // 这条钉住最容易漏的那一步：sub2api 登录页是 Vue，输入框绑 v-model="formData.email"。
        // 只设 el.value 的话 DOM 上看得见字、但 formData.email 仍是空的 —— 提交上去是空邮箱，
        // 而且表现是「明明填了却说邮箱必填」，非常难查。v-model 听的是 input 事件。
        let s = script("https://bestapi.store", "me@x.com", None);
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
        let s = script("https://bestapi.store", "me@x.com", None);
        assert!(!s.contains("type=password"), "选择器不该匹配密码框: {s}");
        assert!(s.contains("'#email'"), "sub2api 的框是 id=email");
        // new-api 那类用 username（实测 LoginRequest.Username，无 email 校验），也要覆盖。
        assert!(s.contains("'#username'"), "多中转站要能兼容 username 形式");
    }

    #[test]
    fn prefill_is_skipped_when_there_is_nothing_to_fill() {
        // 首次登录没有可预填的值。此时脚本仍要正常工作（凭据回传不受影响），
        // 只是不填 —— 而不是填一个空串把用户已输入的东西清掉。
        let s = script("https://bestapi.store", "", None);
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
        let s = script("https://bestapi.store", "a\" + alert(1) + \"", None);
        assert!(!s.contains("\" + alert(1) + \""), "登录标识没被转义: {s}");
    }

    #[test]
    fn prefill_does_not_overwrite_what_the_user_already_typed() {
        // 用户可能正要换个账号登录。已经有值就别覆盖。
        let s = script("https://bestapi.store", "me@x.com", None);
        assert!(
            s.contains("if (el.value)"),
            "已有输入时必须让用户自己的输入胜出: {s}"
        );
    }

    #[test]
    fn aff_code_is_seeded_only_for_sites_in_the_table() {
        // 表里有的站：脚本带上种码那段。
        let seeded = script("https://wawapii.com", "", Some("4PAUD8SSZXG7"));
        assert!(
            seeded.contains("affiliate_referral_code"),
            "表里的站该种码：{seeded}"
        );
        assert!(seeded.contains("4PAUD8SSZXG7"), "码要真的插进去");

        // 表里没有的站（含维护者自己的站）：**整段都不该出现**。
        // 不是「写一个空码」——那会在站点那边存一条 code 为空的脏数据。
        for origin in ["https://bestapi.store", "https://unknown-relay.com"] {
            let bare = script(origin, "", None);
            assert!(
                !bare.contains("affiliate_referral_code"),
                "{origin} 不该有种码那段：{bare}"
            );
        }
    }

    #[test]
    fn seeding_never_overwrites_an_unexpired_code_from_someone_else() {
        // ⭐ 用户可能是**别人邀请来的**（他自己带 `?aff=` 进过这个站，站点已存了那个码）。
        // 无条件覆盖等于抢别人的邀请关系 —— 那不只是不礼貌，是把别人的收益转给自己。
        //
        // 判据要用站点自己的语义（`expiresAt > Date.now()` 才算有效），别自己发明。
        let s = script("https://wawapii.com", "", Some("4PAUD8SSZXG7"));
        assert!(
            s.contains("Number(existing.expiresAt) > Date.now()"),
            "必须按站点的过期语义判「已有码是否仍有效」：{s}"
        );
        assert!(s.contains("if (!stillValid)"), "只在没有有效码时才写：{s}");
    }

    #[test]
    fn seeded_code_carries_the_sites_own_ttl_not_forever() {
        // 写成永久会让一个早已过期的码在站点看来仍然有效 —— 那是给它塞脏数据。
        let s = script("https://wawapii.com", "", Some("4PAUD8SSZXG7"));
        assert!(
            s.contains(&AFFILIATE_REFERRAL_TTL_MS.to_string()),
            "TTL 要与站点一致（30 天）：{s}"
        );
        // 值的形状必须是 `{code, expiresAt}` 的 JSON，不是裸字符串 ——
        // 站点是 `JSON.parse` 它的，裸串会让它整个读取失败。
        assert!(s.contains("JSON.stringify({"), "值必须是 JSON 对象：{s}");
        assert!(s.contains("expiresAt: Date.now() +"), "要带 expiresAt");
    }

    /// 种 aff 码那段整个失效也不该影响凭据回传 —— 两半**完全解耦**。
    ///
    /// ⚠️ 原来这条还断言 `contains("} catch (e) {")` 来验「种码自己兜异常」，
    /// 那是个**假闸**：主脚本里那个字符串本来就无条件出现两次
    /// （读 token 处、劫持 setItem 处）⇒ 恒为真。reviewer 把 aff 那段的
    /// try/catch 整个删掉，测试照样绿。
    ///
    /// 「自己兜异常」那半已挪到 [`each_optional_snippet_catches_its_own_errors`]
    /// ——那里直接测 snippet 函数的输出，断言落在正确的地方。
    #[test]
    fn the_credential_relay_is_independent_of_the_aff_snippet() {
        for aff in [Some("4PAUD8SSZXG7"), None] {
            let s = script("https://wawapii.com", "", aff);
            assert!(s.contains(CREDS_SCHEME), "凭据回传那半必须在");
            assert!(s.contains(AUTH_TOKEN_KEY), "读凭据那半照旧");
        }
    }

    /// ⭐ **注册页横幅不许影响凭据回传。**
    ///
    /// 它是个提示，而凭据回传是这个脚本存在的理由。`promo_snippet` 那次
    /// review 抓出的 P0 正是这一类：一段辅助功能把主流程带崩了
    /// （那次是留空导致 `ReferenceError` ⇒ 定时器永不清除）。
    ///
    /// 横幅这段是**自包含**的（整段 `try { … }`，主脚本里没有对它的调用点），
    /// 与 `aff_snippet` 同一形状。这条闸钉住那个性质。
    #[test]
    fn the_register_banner_never_breaks_the_credential_relay() {
        // 四种组合都验：横幅是无条件生成的，不该被 aff / promo 的有无影响。
        for aff in [Some("4PAUD8SSZXG7"), None] {
            for promo in [Some("LOONGPORT"), None] {
                let s = login_script("https://bestapi.store", "", aff, promo);
                assert!(s.contains(CREDS_SCHEME), "凭据回传那半必须在");
                assert!(s.contains(AUTH_TOKEN_KEY), "读凭据那半照旧");
                assert!(
                    s.contains("loongport-register-hint"),
                    "横幅那段该无条件生成（它自己判在不在 /register）"
                );
            }
        }
    }

    /// 横幅的文案**必须取自页面自己的链接**，不能自带硬编码文案。
    ///
    /// LoongPort 的界面语言与站点的语言可能不同 —— 站点是英文界面时冒出两句中文，
    /// 比没有横幅更糟。这条闸的判据是「脚本里没有中文字面量的文案」：
    /// 它只允许出现在注释里，不允许进 `textContent`。
    ///
    /// 会红的改法：图省事往 `text.textContent = '已有账号？'` 里塞一句话。
    #[test]
    fn the_register_banner_takes_its_wording_from_the_page() {
        let s = register_hint_banner_snippet();
        // 必须从页面上找那个链接。
        assert!(
            s.contains(r#"a[href="/login"]"#),
            "横幅要复用页面自己的「去登录」链接（文案与跳转都靠它）"
        );
        // 文案来自那个链接的 textContent，而不是字面量。
        assert!(
            s.contains("link.textContent"),
            "文案必须取自那个链接，不能自带一份"
        );
        // 点击走站内路由（点现成的链接），不是整页跳 —— 后者会丢 aff 关系。
        assert!(
            s.contains("fresh.click()"),
            "要点现成的 router-link，别用 location.href（整页跳会丢邀请码关系）"
        );
        assert!(
            !s.contains("location.href"),
            "不许整页跳转：那会重跑注入脚本并丢掉 localStorage 里的邀请码关系"
        );
    }

    /// ⭐ **横幅撤掉时必须还原 `body.paddingTop`。**
    ///
    /// 登录窗在拿到凭据后**有意不关**（`commands/relay.rs` 那段「不关窗」：dashboard 上
    /// 有余额与充值入口，用户还要接着用）。所以不还原的话，他会带着一条 40px 的空白条
    /// 浏览登录页、dashboard、充值页 —— 而那条横幅早就不在了。
    #[test]
    fn the_banner_restores_the_body_padding_it_added() {
        let s = register_hint_banner_snippet();
        assert!(
            s.contains("paddedByUs"),
            "要记下「是我们设的 paddingTop」——否则不知道该不该还原（站点自己也可能设过）"
        );
        assert!(
            s.contains("document.body.style.paddingTop = ''"),
            "removeBanner 必须还原 paddingTop"
        );
    }

    /// ⭐ **轮询必须有上限。**
    ///
    /// 这条闸对应我写错的一个前提：原注释说「登录窗的生命周期就是这一次登录，窗口关掉
    /// 定时器随之消失」—— 而窗口**有意不关**，用户想看多久看多久。没有上限意味着那个
    /// 定时器会一直跑下去。
    ///
    /// 主脚本里那个轮询有 600 次上限，这里同一个数量级 —— 横幅只在注册/登录那一小段
    /// 有意义，走到 dashboard 之后它永远不会再显示。
    #[test]
    fn the_banner_poll_is_bounded() {
        let s = register_hint_banner_snippet();
        assert!(
            s.contains("clearInterval"),
            "定时器要能停 —— 登录窗不会自动关，无上限的轮询会一直跑"
        );
        assert!(
            s.contains("polls > 600"),
            "上限与主脚本那个轮询同一个数量级（600 × 500ms = 5 分钟）"
        );
    }

    /// 横幅只在注册页显示，路由切走要撤掉。
    ///
    /// 脚本注一次、SPA 路由切换不重跑（本模块反复踩到的事实）。所以横幅必须
    /// 自己盯路径 —— 否则用户点了「去登录」，横幅跟着留在登录页上，
    /// 说着一句已经不成立的话。
    #[test]
    fn the_register_banner_is_scoped_to_the_register_route() {
        let s = register_hint_banner_snippet();
        assert!(s.contains("'/register'"), "要判当前是不是注册页");
        assert!(
            s.contains("window.location.hash"),
            "hash 路由的站点（`/#/register`）也要认 —— 那时 pathname 是 `/`"
        );
        assert!(s.contains("removeBanner"), "不在注册页时要撤掉横幅");
    }

    #[test]
    fn promo_code_is_prefilled_only_for_sites_that_have_one() {
        // 有码的站：带上预填那段。
        let with = login_script("https://bestapi.store", "", None, Some("LOONGPORT"));
        assert!(with.contains("'#promo_code'"), "要瞄准注册页那个框：{with}");
        assert!(with.contains(r#"PROMO = "LOONGPORT""#), "码要真的插进去");

        // 没有码的站：**整段都不该出现**（不是「填一个空码」）。
        let without = login_script("https://unknown-relay.com", "", None, None);
        assert!(
            !without.contains("#promo_code"),
            "没码的站不该有预填那段：{without}"
        );
        // 但脚本其余部分照旧工作。
        assert!(without.contains(CREDS_SCHEME));
    }

    /// ⭐⭐ **`tryPrefillPromo` 在两种情况下都必须**有定义** —— 否则脚本 throw。**
    ///
    /// review 2026-08-04 抓出的 P0，而 2560 个绿测试全都没发现它：
    /// 所有 promo 用例传的都是 `Some(..)`，而 `None` 那条只断言了
    /// **某些字符串不出现**，从没断言过「那个函数还调得动」。
    ///
    /// ## 为什么这个漏洞的后果远不止「优惠码没填上」
    ///
    /// `tryPrefillPromo()` 在主脚本里被**无条件调用两次**（立即一次 +
    /// 轮询里每轮一次）。定义缺失 ⇒ `ReferenceError` ⇒
    /// 轮询回调在 `clearInterval` 之前就 throw ⇒ **定时器永不清除**。
    /// 凭据回传只是因为 `trySend()` 排在它前面才侥幸活着 —— 顺序上的运气。
    ///
    /// 而且**打中的是常见情况**：`PROMO_CODES` 表里只有一个站，
    /// 其余所有中转站都走 `None`。
    ///
    /// ## 为什么必须断言「定义」而不是「出现过这个名字」
    ///
    /// `contains("tryPrefillPromo")` 会被那两个**调用点**满足 ⇒ 零鉴别力。
    /// 所以断言的是 `function tryPrefillPromo`（带 `function` 关键字）。
    #[test]
    fn the_promo_function_is_always_defined_even_when_there_is_no_code() {
        for (label, code) in [("有码", Some("LOONGPORT")), ("没码", None)] {
            let s = login_script("https://x.com", "", None, code);
            assert!(
                s.contains("function tryPrefillPromo"),
                "{label}的站也必须有 tryPrefillPromo 的**定义** —— \
                 主脚本无条件调用它，缺定义就是 ReferenceError，\
                 而那会让轮询定时器永不清除：{s}"
            );
            // 调用点也还在（这两者一起才构成「调得动」）。
            assert!(
                s.contains("tryPrefillPromo();"),
                "{label}的站要有调用点：{s}"
            );
        }
    }

    /// 没有优惠码时那个空实现**不该带任何 DOM 操作** ——
    /// 它的存在只为「让无条件调用不 throw」，多一个字都是噪音。
    #[test]
    fn the_no_code_stub_does_nothing() {
        let s = login_script("https://x.com", "", None, None);
        assert!(
            s.contains("function tryPrefillPromo() {}"),
            "没码时该是个空函数：{s}"
        );
        // 空实现不该碰 DOM、也不该提优惠码那个框。
        assert!(!s.contains("#promo_code"), "空实现不该有选择器：{s}");
        assert!(!s.contains("PROMO ="), "空实现不该有码变量：{s}");
    }

    /// ⭐ **优惠码必须走 DOM 预填，不能只挂 URL 或只写 localStorage。**
    ///
    /// 事实源（`RegisterView.vue:499-506`）：站点只从 `?promo=` query 读优惠码，
    /// **没有 localStorage 回落**（这一点与 aff 码相反 —— 那个有
    /// `affiliate_referral_code` 键）。而我们跨站内路由（登录页 →「去注册」是
    /// 裸路径 router-link），query 会丢 ⇒ 只剩 DOM 这条路。
    #[test]
    fn promo_prefill_goes_through_the_dom_because_the_site_has_no_storage_fallback() {
        let s = login_script("https://bestapi.store", "", None, Some("LOONGPORT"));
        assert!(
            s.contains("querySelector('#promo_code')"),
            "必须填 DOM：站点的优惠码没有 localStorage 回落，而 query 会在站内跳转时丢：{s}"
        );
        // 必须派事件：v-model 靠它同步 formData，站点的实时校验也挂在 input 上
        // （不派的话那条「+N 赠额」的绿条不出现，用户不知道码生效了）。
        assert!(
            s.contains("new Event('input'"),
            "不派 input 事件等于没填（Vue 的 formData.promo_code 仍是空的）：{s}"
        );
    }

    /// ⭐ **优惠码那段必须每轮都试，不能像登录标识那样填一次就收手。**
    ///
    /// `#promo_code` 只存在于 `/register`。用户可能先落在 `/login`（重登的行就是）、
    /// 点页脚「去注册」才过去 —— 而站内路由跳转**不重跑** `initialization_script`。
    /// 填一次就置标志位的话，这条路径永远填不上。
    #[test]
    fn promo_prefill_keeps_retrying_because_the_field_appears_after_a_route_change() {
        let s = login_script("https://bestapi.store", "", None, Some("LOONGPORT"));
        // 挂在轮询里（而不是只在脚本开头跑一次）。
        assert!(
            s.matches("tryPrefillPromo();").count() >= 2,
            "既要立即试一次、也要挂进轮询：{s}"
        );
        // 收手的判据按**元素身份**记，而不是一次性的布尔标志（那跨不了路由）。
        assert!(
            s.contains("new WeakSet()") && s.contains("promoFilled"),
            "要按元素身份记「填过了」，重挂的新元素才还能填：{s}"
        );
    }

    /// ⭐ **用户必须能清掉这个码。**
    ///
    /// codex review 2026-08-04 抓出的真缺陷：收手判据原本是「框里有没有值」，
    /// 于是用户删空之后 500ms 又被填回去 —— 他换个非空码可以，
    /// 唯独「不想要任何码」做不到。而站点的 `handleInput` 明确允许空值
    /// （清掉校验态，`RegisterView.vue:598`）。
    ///
    /// 那是把「提供便利」变成「强加」，所以是缺陷不是取舍。
    #[test]
    fn the_user_can_clear_the_promo_code_and_it_stays_cleared() {
        let s = login_script("https://bestapi.store", "", None, Some("LOONGPORT"));
        // 判据必须**不是**「框里有没有值」那一种（那正是会填回去的那版）。
        assert!(
            s.contains("promoFilled.has(el)"),
            "收手要按元素身份判，否则用户清空后会被填回：{s}"
        );
        // 填之前就记下这个元素 —— 否则「填过了」这件事只能靠值来推断，
        // 又回到那个坑里。
        assert!(s.contains("promoFilled.add(el)"), "填过的元素要记下来：{s}");

        // ⭐ **`add` 必须在「用户已有输入」那条出路之**前** —— 顺序本身是防 bug 的。
        //
        // 若写成「只在真的填了之后才 add」，那么用户已有输入的那一轮不记 ⇒
        // 每轮都重新走到 `if (el.value) return;` ⇒ 他把框清空的那一刻
        // `el.value` 变空 ⇒ 当轮就被我们填上。绕回原来那个 bug。
        let add_at = s.find("promoFilled.add(el)").expect("有 add");
        let bail_at = s.find("if (el.value) return;").expect("有那条出路");
        assert!(
            add_at < bail_at,
            "add 必须在「用户已有输入就退出」之前，否则用户清空的那一刻会被填回：{s}"
        );
    }

    /// 「填过了」必须**以元素为键**记，不能是一个一次性布尔标志 ——
    /// 后者会让用户从 `/login` 跳回 `/register` 时填不上
    /// （那两个是独立路由组件，来回跳会 remount 出新元素）。
    ///
    /// ## ⚠️ 这条原来是弱闸（review 抓出）
    ///
    /// 原写法是 `!s.contains("promoDone")` —— 只禁了**一个具体拼写**。
    /// reviewer 把标志改名成 `promoOnce` 就绕过去了。
    ///
    /// 现在断言的是那个容器**真的拿元素当键**（`has(el)` / `add(el)`），
    /// 那是「按身份记」与「布尔标志」的结构区别所在，改名绕不过。
    ///
    /// 而「重挂之后确实还能填」这个**行为**本身由
    /// `tests/lib/loginScriptRuns.test.ts` 真跑一遍来守 —— 字符串断言
    /// 至多能验结构，验不了行为。
    #[test]
    fn the_filled_marker_is_keyed_by_element_not_a_one_shot_flag() {
        let s = login_script("https://bestapi.store", "", None, Some("LOONGPORT"));
        assert!(
            s.contains("WeakSet"),
            "容器要用 WeakSet（元素卸载后不该留强引用）：{s}"
        );
        // 关键是**拿元素当键**：布尔标志做不到这件事。
        assert!(s.contains(".has(el)"), "查的时候要按元素查：{s}");
        assert!(s.contains(".add(el)"), "记的时候要记元素：{s}");
    }

    /// 两段附加功能（种 aff 码、预填优惠码）都必须**自己兜异常**。
    ///
    /// ## ⚠️ 断言必须落在 snippet 上，不能落在整段脚本上（review 抓出的假闸）
    ///
    /// 原来这两条测的是 `login_script(..).contains("} catch (e) {")` ——
    /// 而主脚本里那个字符串**本来就无条件出现两次**
    /// （读 token 那处、劫持 setItem 那处）⇒ 断言恒为真、**零鉴别力**。
    /// reviewer 把 snippet 里的 try/catch 整个删掉，测试照样绿。
    ///
    /// 所以改成直接测那两个 snippet 函数：断言精确落在它们自己的输出上。
    /// 同理 `contains(CREDS_SCHEME)` 那句也删了 —— 它测的是「主脚本还在」，
    /// 与「这段 snippet 自己兜没兜异常」无关，混在一条测试里只会掩盖前者失效。
    #[test]
    fn each_optional_snippet_catches_its_own_errors() {
        for (what, snippet) in [
            ("种 aff 码", aff_seed_snippet("AFF12345678")),
            ("预填优惠码", promo_prefill_snippet("LOONGPORT")),
        ] {
            assert!(
                snippet.contains("catch (e)"),
                "{what}那段要自己兜异常 —— 它是附加好处，不该打断登录：{snippet}"
            );
            // 兜异常的 `catch` 块里**什么都不做**（不重试、不上报）。
            // 有 `catch` 但里面又抛一次等于没兜。
            assert!(
                !snippet.contains("throw"),
                "{what}的 catch 里不该再抛：{snippet}"
            );
        }
    }

    /// 优惠码那段整个失效也不该影响凭据回传 —— 两半**完全解耦**。
    ///
    /// 与上面那条分开：这条测的是「主脚本那半还在」，那是另一件事。
    /// 合成一条会让其中一半失效时另一半的断言仍把测试染绿。
    #[test]
    fn the_credential_relay_is_independent_of_the_promo_snippet() {
        for promo in [Some("LOONGPORT"), None] {
            let s = login_script("https://bestapi.store", "", None, promo);
            assert!(s.contains(CREDS_SCHEME), "凭据回传那半必须在");
            assert!(s.contains(AUTH_TOKEN_KEY), "读凭据那半必须在");
        }
    }

    #[test]
    fn promo_prefill_json_encodes_the_code_so_quotes_cannot_break_out() {
        // 码来自人手录的表（或远端配置）—— 同样不能直接插进脚本。
        let s = login_script(
            "https://bestapi.store",
            "",
            None,
            Some("X\" + alert(1) + \""),
        );
        assert!(!s.contains("\" + alert(1) + \""), "优惠码没被转义: {s}");
    }

    /// ⭐ **两个码可以同时带** —— 它们是服务端两个不同的字段。
    ///
    /// 这条钉住「加 promo 没把 aff 挤掉」：早期实现若复用同一个 snippet 变量，
    /// 后者会覆盖前者，而后果是维护者的返利静默消失。
    #[test]
    fn both_codes_can_be_carried_at_once() {
        let s = login_script(
            "https://some-relay.com",
            "",
            Some("4PAUD8SSZXG7"),
            Some("LOONGPORT"),
        );
        assert!(s.contains("affiliate_referral_code"), "aff 那段要在");
        assert!(s.contains("4PAUD8SSZXG7"), "aff 码要在");
        assert!(s.contains("#promo_code"), "promo 那段要在");
        assert!(s.contains("LOONGPORT"), "promo 码要在");
    }

    /// ⭐ **落哪个页面由「这一行登录过没有」决定**（2026-08-04 维护者拍板）。
    ///
    /// 「刚开始大部分都是新户」—— 新加一个站的人通常还没有那个站的账号。
    /// 而重登的行落注册页是明确的退步（那个预填标识就是给登录框准备的）。
    #[test]
    fn a_brand_new_site_lands_on_register_but_a_relogin_lands_on_login() {
        // 没登录过（`login_identifier` 空）⇒ 注册页。
        assert_eq!(
            login_url("https://bestapi.store", ""),
            "https://bestapi.store/register",
            "新加的站落注册页 —— 新户不该多点一次「去注册」"
        );
        // 登录过 ⇒ 登录页。
        assert_eq!(
            login_url("https://bestapi.store", "me@x.com"),
            "https://bestapi.store/login",
            "重登落登录页 —— 预填的标识是给登录框用的"
        );
    }

    /// 判据必须是 `login_identifier`，不是别的东西。
    ///
    /// 这条守的是「同一事实别存两份」：那个值**正是**成功登录后才写进去的
    /// （`commands/relay.rs` 里 `op.login_identifier = account.email`），
    /// 空与非空已经精确表达了「登录过没有」。有人若新加一个布尔列来判，
    /// 就会有两个可能不同步的真相源。
    #[test]
    fn the_login_url_switch_keys_off_the_prefill_identifier() {
        // 同一个站、只有标识不同 ⇒ 落点必须不同。
        assert_ne!(
            login_url("https://x.com", ""),
            login_url("https://x.com", "me@x.com"),
            "标识的有无必须真的改变落点"
        );
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

/// 把生成的脚本导出到文件，供 Node 那侧真的执行一遍。
///
/// ## 为什么需要它（2026-08-04 加，一个 P0 逼出来的）
///
/// 本模块生成的是**另一门语言的代码**，而 Rust 侧的测试全是字符串断言 ——
/// 它们能验「该出现的出现了」，验不了「这段 JS 跑得起来」。
/// 那个 P0 正是这样溜过 2562 个绿测试的：`tryPrefillPromo` 的调用点在，
/// 定义不在，所有字符串断言全过，而脚本一执行就 `ReferenceError`。
///
/// 所以有了这条：`cargo test --lib -- --ignored export_login_scripts` 把两种
/// 变体写到 `target/login-script-*.js`，再由 `tests/lib/loginScriptRuns.test.ts`
/// 在 Node 的 `vm` 里真的跑。
///
/// **`#[ignore]` 是有意的**：它不是断言什么，只是给前端那条测试准备素材。
/// CI 里由那条 npm 测试负责（它自己会调这个导出）。
#[cfg(test)]
#[test]
#[ignore = "不是断言，只导出素材给 tests/lib/loginScriptRuns.test.ts"]
fn export_login_scripts() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&dir).expect("建 target 目录");
    for (name, promo) in [("with-promo", Some("LOONGPORT")), ("no-promo", None)] {
        let s = login_script(
            "https://bestapi.store",
            "me@x.com",
            Some("AFF12345678"),
            promo,
        );
        std::fs::write(dir.join(format!("login-script-{name}.js")), s).expect("写脚本");
    }
    // 浏览器代拉 API 请求的脚本同样要执行验证（`loginScriptRuns.test.ts`）。
    let request = reqwest::Client::new()
        .get("https://bestapi.store/api/v1/user/profile?x=1")
        .bearer_auth("tok-secret")
        .build()
        .expect("构建请求");
    let api = crate::relay::browser_bridge::api_fetch_script(&request, "api-0");
    std::fs::write(dir.join("api-fetch-script.js"), api).expect("写脚本");

    // POST 分支（带 body + 幂等头）同样要执行验证 —— 那行 `init.body = ...` 是
    // 按请求条件生成的字面量，GET 脚本里不存在，不另导出就永远验不到。
    let create = reqwest::Client::new()
        .post("https://bestapi.store/api/v1/keys")
        .json(&serde_json::json!({ "name": "provision:test", "group_id": 3 }))
        .header("Idempotency-Key", "idem-1")
        .build()
        .expect("构建请求");
    let api_post = crate::relay::browser_bridge::api_fetch_script(&create, "api-1");
    std::fs::write(dir.join("api-fetch-script-post.js"), api_post).expect("写脚本");
}
