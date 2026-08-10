//! 协议无关的浏览器辅助站点发现。
//!
//! WebView 只负责复用用户刚刚完成网页验证的同源会话，并抓取注册表中的候选端点；
//! **不在浏览器层猜站点类型**。原始响应回到 Rust 后，才由各协议 detector 严格判定。
//! 将来接 new-api 时，只需增加候选与 detector，不改“打开网页 → 用户验证”的共用流程。

use base64::Engine;
use serde::Serialize;

use crate::error::AppError;
use crate::relay::api;

pub const PROBE_SCHEME: &str = "loongport-probe";
const SUB2API_CANDIDATE_ID: &str = "sub2api";
const MAX_PROBE_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProbeCandidate {
    pub id: &'static str,
    pub path: &'static str,
}

pub const PROBE_CANDIDATES: &[ProbeCandidate] = &[ProbeCandidate {
    id: SUB2API_CANDIDATE_ID,
    path: "/api/v1/settings/public",
}];

/// 严格 detector 已确认的协议及其协议专属公开信息。
///
/// 通用发现层不假设不同协议共享 DTO；将来加入 new-api 时新增 enum variant 即可。
#[derive(Debug, Clone)]
pub enum DetectedSite {
    Sub2Api(api::PublicSettings),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    pub candidate_id: String,
    pub body: String,
}

/// 生成注入所有页面的协议无关探测脚本。
///
/// 脚本只在目标 origin 上工作；验证页、注册页、登录页都走同一份代码。它不认识任何
/// 验证产品或 HTTP 状态，只轮询候选端点并把 JSON-like 响应交给 Rust detector。
pub fn browser_probe_script(site_origin: &str, candidates: &[ProbeCandidate]) -> String {
    let expected_origin = serde_json::to_string(site_origin).expect("origin can be JSON encoded");
    let candidates =
        serde_json::to_string(candidates).expect("probe candidates can be JSON encoded");

    format!(
        r#"
(() => {{
  const expectedOrigin = {expected_origin};
  const candidates = {candidates};
  const callbackScheme = '{PROBE_SCHEME}';
  const sentBodies = new Map();

  function b64url(value) {{
    const bytes = new TextEncoder().encode(value);
    let binary = '';
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
  }}

  async function probe() {{
    if (window.location.origin !== expectedOrigin) return;

    for (const candidate of candidates) {{
      try {{
        const response = await fetch(candidate.path, {{
          credentials: 'include',
          cache: 'no-store',
          headers: {{ Accept: 'application/json' }},
        }});
        const body = await response.text();
        const trimmed = body.trim();
        if (!trimmed || (trimmed[0] !== '{{' && trimmed[0] !== '[')) continue;
        if (new TextEncoder().encode(body).length > {MAX_PROBE_BODY_BYTES}) continue;
        if (sentBodies.get(candidate.id) === body) continue;
        sentBodies.set(candidate.id, body);
        window.location.href = callbackScheme + '://response?id=' +
          encodeURIComponent(candidate.id) + '&d=' + b64url(body);
      }} catch (_) {{
        // 页面仍可能处于验证或跳转阶段；下一轮继续，不在浏览器层解释失败原因。
      }}
    }}
  }}

  void probe();
  setInterval(() => void probe(), 1500);
}})();
"#
    )
}

/// 解析 WebView 的探测回传导航。`None` 表示普通网页导航，应继续放行。
pub fn parse_probe_navigation(url: &url::Url) -> Option<Result<ProbeResponse, AppError>> {
    if url.scheme() != PROBE_SCHEME {
        return None;
    }

    Some((|| {
        let candidate_id = url
            .query_pairs()
            .find(|(key, _)| key == "id")
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Config("站点探测回传缺少 candidate id".into()))?;
        let encoded = url
            .query_pairs()
            .find(|(key, _)| key == "d")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| AppError::Config("站点探测回传缺少响应正文".into()))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| AppError::Config(format!("站点探测回传无法解码: {e}")))?;
        if bytes.len() > MAX_PROBE_BODY_BYTES {
            return Err(AppError::Config("站点探测回传正文过大".into()));
        }
        let body = String::from_utf8(bytes)
            .map_err(|e| AppError::Config(format!("站点探测回传不是 UTF-8: {e}")))?;

        Ok(ProbeResponse { candidate_id, body })
    })())
}

/// 用原生 HTTP 跑候选注册表的 fast path。
///
/// 任何传输失败、非成功状态或 detector 不匹配都只意味着“这个候选没有识别出来”；
/// 不在这里把某次失败宣判成站点类型。全部候选都不匹配时，调用方可切到可见 WebView。
pub async fn probe_site(site_origin: &str) -> Result<DetectedSite, AppError> {
    let client = api::build_client()?;
    for candidate in PROBE_CANDIDATES {
        let url = format!("{site_origin}{}", candidate.path);
        let Ok(response) = client.get(&url).send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(body) = response.text().await else {
            continue;
        };
        if let Some(site) = detect_site(candidate.id, &body) {
            return Ok(site);
        }
    }

    Err(AppError::Config(format!(
        "{site_origin} 的原生探测未识别出受支持的站点协议"
    )))
}

/// 在 Rust 侧按候选 id 分派严格 detector。
///
/// 未知候选或形状不匹配都返回 `None`。浏览器层永远不根据端点存在、HTTP 状态或通用字段
/// 直接认定协议，因此 new-api 的响应不会被 sub2api detector 误收。
pub fn detect_site(candidate_id: &str, body: &str) -> Option<DetectedSite> {
    match candidate_id {
        SUB2API_CANDIDATE_ID => api::parse_sub2api_public_settings(body)
            .ok()
            .map(DetectedSite::Sub2Api),
        _ => None,
    }
}

#[cfg(test)]
fn probe_callback_url(candidate_id: &str, body: &str) -> url::Url {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body);
    url::Url::parse(&format!(
        "{PROBE_SCHEME}://response?id={candidate_id}&d={encoded}"
    ))
    .expect("test callback URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_probe_script_is_protocol_neutral_and_supports_multiple_candidates() {
        let candidates = [
            ProbeCandidate {
                id: "sub2api",
                path: "/api/v1/settings/public",
            },
            ProbeCandidate {
                id: "future",
                path: "/api/status",
            },
        ];
        let script = browser_probe_script("https://relay.example", &candidates);

        assert!(script.contains("/api/v1/settings/public"));
        assert!(script.contains("/api/status"));
        assert!(script.contains("credentials: 'include'"));
        assert!(script.contains(PROBE_SCHEME));
        assert!(script.contains("window.location.origin"));
        assert!(!script.contains("Cloudflare"));
        assert!(!script.contains("cf_clearance"));
        assert!(!script.contains("403"));
    }

    #[test]
    fn probe_navigation_round_trips_candidate_and_raw_json() {
        let body = r#"{"code":0,"data":{"version":"1"}}"#;
        let url = probe_callback_url("sub2api", body);
        let parsed = parse_probe_navigation(&url)
            .expect("probe scheme")
            .expect("valid callback");
        assert_eq!(parsed.candidate_id, "sub2api");
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn detector_registry_does_not_accept_new_api_as_sub2api() {
        let body = r#"{"success":true,"data":{"version":"1","system_name":"new-api"}}"#;
        assert!(detect_site("sub2api", body).is_none());
    }
}
