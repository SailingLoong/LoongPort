//! 登录窗代拉 API 请求的通用浏览器桥。
//!
//! 站点启用 Cloudflare 这类**指纹级**防护（非浏览器 HTTP 栈一律 403 HTML；加头、
//! 加 cookie、换 HTTP 版本都过不了，`remote-config/README.md` 里那台 `api.aijws.com`
//! 就是实例）时，唯一能过防护的通道是真实浏览器 —— 而登录窗本身就是真实浏览器，
//! 且登录成功后**故意不关**（页面刚跳到 dashboard，用户还要看余额、充值）。
//!
//! 本模块就是那条通道：[`super::api::Client`] 撞上防护层时，把**同一份请求**（method +
//! URL + 头 + body）原样搬进登录窗页面上下文里 fetch，响应经自定义 scheme
//! `loongport-creds://api-<id>` 回传（与凭据回传同一条传输通道，用 host 区分）。
//!
//! 回传按请求 id 认领（[`BrowserBridge`]）：发起方注册一个 oneshot 槽位拿到 id，
//! 窗口导航回调按 id 送回。发起方不持有窗口、窗口也不认识发起方 —— 所以登录后自动
//! 备 key（provision）这类**在登录命令返回之后**才跑的流程，也能借同一扇窗代拉。

use crate::relay::login::CREDS_SCHEME;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::oneshot;

/// 浏览器代拉一次请求的回传载荷。
///
/// 只做传输：状态 + 正文原样送回，**业务分类（401 / 信封解析）归
/// [`super::api`] 那一份逻辑** —— 在这里复刻就是第二份事实源。
#[derive(Debug, Clone)]
pub struct BridgeResponse {
    /// HTTP 状态码。负数是页面内 fetch 的传输层错误：
    /// `-1` 网络错 / `-2` 脚本错 / `-3` 超时 / `-4` 页面 origin 已离开站点。
    pub status: i64,
    /// `content-type`（判「403 + HTML = 防护层拦截」用）。
    pub content_type: String,
    /// 响应正文原文。
    pub body: String,
}

/// 回传 URL 的 host 前缀：`loongport-creds://api-<id>`。
///
/// 与凭据回传的 host `ok` 分开：载荷形状不同（凭据是一组字段、代拉是 `{status, body}`），
/// 合用一个 host 会让解析器按错形状解。
const CALLBACK_HOST_PREFIX: &str = "api-";

/// Rust 侧等页面 fetch + 回传的最长秒数。
///
/// 只在 reqwest 被指纹级防护拦下（403 HTML）时才会走到这条路径；正常站点 reqwest
/// 一次往返就拿到响应，不会等这个超时。5 秒给真实浏览器留够往返，又不会让流程在
/// 窗口已关（回传永远不来）时卡成几分钟超时。
pub const FETCH_TIMEOUT_SECS: u64 = 5;

/// 全 app 一份的浏览器代拉回传调度器。
///
/// 挂在 [`crate::store::AppState`] 上而不是随登录流程走：provision 等流程在登录命令
/// **返回之后**才跑，那时登录流程的局部通道早已 drop，只有 app 级注册表还在。
#[derive(Default)]
pub struct BrowserBridge {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<BridgeResponse, String>>>>,
    next_id: AtomicU64,
}

impl BrowserBridge {
    /// 注册一次代拉的收件槽，返回要写进回传 URL 的请求 id。
    pub fn register(&self, tx: oneshot::Sender<Result<BridgeResponse, String>>) -> String {
        let id = format!(
            "{CALLBACK_HOST_PREFIX}{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        self.pending
            .lock()
            .expect("bridge 锁")
            .insert(id.clone(), tx);
        id
    }

    /// 从一次窗口导航里认领代拉回传。
    ///
    /// 返回 `true` = 这是代拉回传且已处理（调用方应拦下这次导航）；`false` = 不是，
    /// 按普通导航放行。载荷解不开也按「已处理」收下（错误会送回发起方），
    /// 不会把坏 URL 放行进页面。
    pub fn handle_navigation(&self, url: &url::Url) -> bool {
        if url.scheme() != CREDS_SCHEME {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        if !host.starts_with(CALLBACK_HOST_PREFIX) {
            return false;
        }
        // 以完整 id（含前缀）为注册键 —— 不能 strip 掉前缀再查，否则永远查不中。
        let id = host.to_string();
        let result = url
            .query_pairs()
            .find(|(k, _)| k == "d")
            .map(|(_, v)| v.into_owned())
            .ok_or_else(|| "代拉回传缺少数据".to_string())
            .and_then(|encoded| {
                decode_payload(&encoded).ok_or_else(|| "代拉回传的数据解不开".to_string())
            });
        if let Some(tx) = self.pending.lock().expect("bridge 锁").remove(&id) {
            let _ = tx.send(result);
        }
        true
    }

    /// 撤销一次已注册但不会再等回传的请求（如脚本注入失败）。
    pub fn forget(&self, id: &str) {
        self.pending.lock().expect("bridge 锁").remove(id);
    }
}

/// 生成「让登录窗在页面上下文里同源重放一次 API 请求并回传」的注入脚本。
///
/// 请求从 `request` 本身取（method / 完整 URL / 头 / body）—— 代拉必须与直连是
/// **同一份请求**，由 [`super::api::Client::send`] 把原请求递进来，这里不重拼
/// （重拼就是第二份事实源，多拼错一个头就多一种「浏览器能过、代拉过不了」的分叉）。
///
/// ## 为什么 403 HTML 要重试（2026-08-13 实测加）
///
/// 这类防护对真实浏览器也是**按会话概率性放行**：同一台 Chrome 里，同一窗口先后两次
/// fetch 同一个端点，可能一次 200 一次 403（`api.aijws.com` 实测）。登录窗此刻是
/// **已经登录成功**的信任会话（SPA 自己刚拉过 profile 渲染 dashboard），大多一次就过，
/// 但偶发 403 会让用户白重登一次 —— 所以只对「403 + HTML」重试，最多 3 次、间隔 1 秒，
/// 与 [`super::discovery`] 浏览器探针「轮询直到成功」是同一个思路。真正的 API 错误
/// （401 / 200 + 业务 code）不重试，原样回传。
pub fn api_fetch_script(request: &reqwest::Request, req_id: &str) -> String {
    let origin = request.url().origin().ascii_serialization();
    let origin_literal = serde_json::to_string(&origin).expect("origin 可 JSON 编码");
    let url_literal = serde_json::to_string(request.url().as_str()).expect("url 可 JSON 编码");
    let method_literal =
        serde_json::to_string(request.method().as_str()).expect("method 可 JSON 编码");
    let headers_literal =
        serde_json::to_string(&headers_object(request.headers())).expect("headers 可 JSON 编码");
    let body_decl = match request.body().and_then(|body| body.as_bytes()) {
        Some(bytes) => format!(
            "init.body = {};",
            serde_json::to_string(&String::from_utf8_lossy(bytes)).expect("body 可 JSON 编码")
        ),
        None => String::new(),
    };
    format!(
        r#"(function () {{
  'use strict';

  // 只在顶层 frame 跑（与 `login_script` 同一条规则）：同源 iframe 会让脚本多执行一份。
  if (window.top !== window.self) return;

  var ALLOWED_ORIGIN = {origin_literal};
  // 页面可能已被带到别的 origin（登录窗是全局单例，用户可能已把它留给别的站），
  // 相对 fetch 会打错站。与 `login_script` 那条 early-return 同理：宁可明确失败回传，
  // 也不静默拉错站。
  if (window.location.origin !== ALLOWED_ORIGIN) {{
    send(-4, '页面 origin 已离开站点');
    return;
  }}

  var sent = false;

  function send(status, body, ct) {{
    if (sent) return;
    sent = true;
    var payload = JSON.stringify({{ status: status, body: String(body || ''), ct: String(ct || '') }});
    // 顶层导航（`login_script` 里论证过不能改用 iframe：CSP 会拦）——
    // Rust 侧 `on_navigation` 收到后拦下，页面内容原样留着。
    window.location.href = '{CREDS_SCHEME}://{CALLBACK_HOST_PREFIX}{req_id}?d=' + b64url(payload);
  }}

  function b64url(s) {{
    var bytes = new TextEncoder().encode(s);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  var attempts = 0;
  var MAX_ATTEMPTS = 3;
  var RETRY_DELAY_MS = 1000;

  function attempt() {{
    attempts++;
    var init = {{ method: {method_literal}, headers: {headers_literal}, credentials: 'include' }};
    {body_decl}
    fetch({url_literal}, init)
      .then(function (r) {{
        return r.text().then(function (t) {{
          return {{ status: r.status, ct: r.headers.get('content-type') || '', body: t }};
        }});
      }})
      .then(function (res) {{
        // 防护层偶发拦截的形态：403 + HTML。只对这种情况重试；
        // API 自己的错误（401 / 200 + 业务 code）原样回传。
        var blocked = res.status === 403 && res.ct.indexOf('html') !== -1;
        if (blocked && attempts < MAX_ATTEMPTS) {{
          setTimeout(attempt, RETRY_DELAY_MS);
        }} else {{
          send(res.status, res.body, res.ct);
        }}
      }})
      .catch(function (e) {{ send(-1, e && e.message ? e.message : String(e), ''); }});
  }}

  try {{
    attempt();
  }} catch (e) {{
    send(-2, e && e.message ? e.message : String(e), '');
  }}

  // 页面里 fetch 卡住时不能让调用方干等 —— 5 秒兜底后回传超时。
  setTimeout(function () {{ send(-3, '浏览器代拉请求超时', ''); }}, 5000);
}})();
"#
    )
}

/// 把 `HeaderMap` 摊平成 JSON 对象（同名多值头取最后一条；我们自己的请求都是单值头）。
fn headers_object(
    headers: &reqwest::header::HeaderMap,
) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        let name = name.as_str();
        let Ok(value) = value.to_str() else { continue };
        map.insert(
            name.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    map
}

fn decode_payload(encoded: &str) -> Option<BridgeResponse> {
    let json = crate::relay::login::decode_b64url(encoded)?;
    #[derive(serde::Deserialize)]
    struct Raw {
        status: i64,
        #[serde(default)]
        body: String,
        #[serde(default)]
        ct: String,
    }
    let raw: Raw = serde_json::from_str(&json).ok()?;
    Some(BridgeResponse {
        status: raw.status,
        content_type: raw.ct,
        body: raw.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::login::CREDS_CALLBACK_HOST;

    /// 与生产解码器同构的编码（与 `login.rs` 测试同一份逆运算，避免各带一份）。
    fn callback_host_url(host: &str, payload: &str) -> url::Url {
        let mut bin = String::new();
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

    #[test]
    fn script_is_self_contained_and_origin_guarded() {
        let request = reqwest::Client::new()
            .get("https://relay.example/api/v1/user/profile?x=1")
            .bearer_auth("tok-secret")
            .build()
            .expect("构建请求");
        let script = api_fetch_script(&request, "api-7");
        assert!(
            script.contains("https://relay.example"),
            "origin 字面量要在"
        );
        assert!(script.contains("tok-secret"), "token 要嵌入脚本");
        assert!(
            script.contains("https://relay.example/api/v1/user/profile?x=1"),
            "完整 URL 要嵌入脚本（含 query）"
        );
        assert!(script.contains("api-7"), "回传要带请求 id");
        assert!(script.contains(CREDS_SCHEME), "回传要走自定义 scheme");
        assert!(script.contains("credentials: 'include'"), "要带会话 cookie");
        assert!(
            script.contains("window.location.origin !== ALLOWED_ORIGIN"),
            "origin 守卫要在"
        );
        assert!(
            script.contains("res.ct.indexOf('html') !== -1"),
            "403 + HTML 重试判据要在"
        );
    }

    #[test]
    fn script_carries_post_body_and_idempotency_header() {
        let request = reqwest::Client::new()
            .post("https://relay.example/api/v1/keys")
            .json(&serde_json::json!({ "name": "a", "group_id": 3 }))
            .header("Idempotency-Key", "idem-1")
            .build()
            .expect("构建请求");
        let script = api_fetch_script(&request, "api-1");
        assert!(script.contains("init.body ="), "POST 要有 body");
        assert!(script.contains(r#"\"name\":\"a\""#), "body 内容要在");
        assert!(
            script.contains("idempotency-key"),
            "幂等头要带过去（头名大小写不敏感）"
        );
        assert!(script.contains("\"idem-1\""), "幂等头值要在");
    }

    #[test]
    fn navigation_roundtrips_a_response() {
        let bridge = BrowserBridge::default();
        let (tx, mut rx) = oneshot::channel();
        let req_id = bridge.register(tx);
        assert!(req_id.starts_with(CALLBACK_HOST_PREFIX));

        let payload = format!(
            r#"{{"status":200,"body":{},"ct":"application/json"}}"#,
            serde_json::to_string(r#"{"code":0,"data":{"id":7}}"#).unwrap()
        );
        let url = callback_host_url(&req_id, &payload);
        assert!(bridge.handle_navigation(&url), "代拉回传要认领");
        let response = rx.try_recv().expect("回传要送达").expect("载荷要解出");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"id\":7"));
    }

    #[test]
    fn navigation_leaves_foreign_urls_alone() {
        let bridge = BrowserBridge::default();
        // 普通 http 导航 / 凭据回传（host=ok）都不是代拉回传。
        assert!(
            !bridge.handle_navigation(&url::Url::parse("https://relay.example/dashboard").unwrap())
        );
        assert!(!bridge.handle_navigation(&callback_host_url(CREDS_CALLBACK_HOST, "x")));
    }

    #[test]
    fn malformed_callbacks_error_out_instead_of_panicking() {
        let bridge = BrowserBridge::default();
        let (tx, mut rx) = oneshot::channel();
        let req_id = bridge.register(tx);

        // status 类型不对 / 数据解不开：Error 送回，而不是 panic 或吞掉。
        let wrong_shape = callback_host_url(&req_id, r#"{"status":"x"}"#);
        assert!(bridge.handle_navigation(&wrong_shape));
        let err = rx
            .try_recv()
            .expect("回传要送达")
            .expect_err("坏载荷要报错");
        assert!(!err.is_empty());

        // 没注册过的 id（已超时被 forget）：当作已处理，不 panic。
        let stale = callback_host_url("api-999", "whatever");
        assert!(bridge.handle_navigation(&stale));
    }

    #[test]
    fn forget_removes_pending_slot() {
        let bridge = BrowserBridge::default();
        let (tx, _rx) = oneshot::channel();
        let req_id = bridge.register(tx);
        bridge.forget(&req_id);
        // 已 forget 的回传没有接收方，仍按已处理返回（不 panic）。
        let stale = callback_host_url(&req_id, r#"{"status":200,"body":"x","ct":"json"}"#);
        assert!(bridge.handle_navigation(&stale));
    }
}
