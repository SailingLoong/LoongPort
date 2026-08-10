//! 协议无关的浏览器辅助站点发现。
//!
//! WebView 只负责复用用户刚刚完成网页验证的同源会话，并抓取注册表中的候选端点；
//! **不在浏览器层猜站点类型**。原始响应回到 Rust 后，才由各协议 detector 严格判定。
//! 将来接 new-api 时，只需增加候选与 detector，不改“打开网页 → 用户验证”的共用流程。

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::relay::api;
use crate::relay::newapi;

pub const PROBE_SCHEME: &str = "loongport-probe";
const SUB2API_CANDIDATE_ID: &str = "sub2api";
const NEWAPI_CANDIDATE_ID: &str = "newapi";
const MAX_PROBE_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Sub2Api,
    NewApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeCandidate {
    pub id: &'static str,
    pub path: &'static str,
}

pub const PROBE_CANDIDATES: &[ProbeCandidate] = &[
    ProbeCandidate {
        id: SUB2API_CANDIDATE_ID,
        path: "/api/v1/settings/public",
    },
    ProbeCandidate {
        id: NEWAPI_CANDIDATE_ID,
        path: "/api/status",
    },
];

/// 严格 detector 已确认的协议及其协议专属公开信息。
///
/// 通用发现层不假设不同协议共享 DTO；将来加入 new-api 时新增 enum variant 即可。
#[derive(Debug, Clone)]
pub enum DetectedSite {
    Sub2Api(api::PublicSettings),
}

#[derive(Debug, Clone)]
pub enum DetectedBackend {
    Sub2Api(api::PublicSettings),
    NewApi(newapi::Status),
}

impl DetectedBackend {
    pub fn backend_kind(&self) -> BackendKind {
        match self {
            Self::Sub2Api(_) => BackendKind::Sub2Api,
            Self::NewApi(_) => BackendKind::NewApi,
        }
    }
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
    match discover_site(site_origin).await? {
        DetectedBackend::Sub2Api(settings) => Ok(DetectedSite::Sub2Api(settings)),
        DetectedBackend::NewApi(_) => Err(AppError::Config(format!(
            "{site_origin} 是 newapi 站点，当前命令入口尚未接入该协议"
        ))),
    }
}

/// 原生 HTTP 探测所有候选，再复用 WebView 原始回传使用的同一收敛规则。
pub async fn discover_site(site_origin: &str) -> Result<DetectedBackend, AppError> {
    let client = api::build_client()?;
    let mut responses = Vec::new();

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
        responses.push(ProbeResponse {
            candidate_id: candidate.id.to_string(),
            body,
        });
    }

    converge_probe_responses(&responses)
}

/// 在 Rust 侧按候选 id 分派严格 detector。
///
/// 未知候选或形状不匹配都返回 `None`。浏览器层永远不根据端点存在、HTTP 状态或通用字段
/// 直接认定协议，因此 new-api 的响应不会被 sub2api detector 误收。
pub fn detect_candidate(candidate_id: &str, body: &str) -> Option<DetectedBackend> {
    match candidate_id {
        SUB2API_CANDIDATE_ID => api::parse_sub2api_public_settings(body)
            .ok()
            .map(DetectedBackend::Sub2Api),
        NEWAPI_CANDIDATE_ID => newapi::parse_status(body).ok().map(DetectedBackend::NewApi),
        _ => None,
    }
}

/// 旧命令层的兼容入口：只暴露现有 sub2api 结果，不把 newapi 伪装成 sub2api。
pub fn detect_site(candidate_id: &str, body: &str) -> Option<DetectedSite> {
    match detect_candidate(candidate_id, body)? {
        DetectedBackend::Sub2Api(settings) => Some(DetectedSite::Sub2Api(settings)),
        DetectedBackend::NewApi(_) => None,
    }
}

/// 汇总所有原始候选响应，去重后严格收敛为一个协议结果。
pub fn converge_probe_responses(responses: &[ProbeResponse]) -> Result<DetectedBackend, AppError> {
    let mut detected = Vec::new();
    for response in responses {
        let Some(candidate) = detect_candidate(&response.candidate_id, &response.body) else {
            continue;
        };
        if detected
            .iter()
            .any(|item: &DetectedBackend| item.backend_kind() == candidate.backend_kind())
        {
            continue;
        }
        detected.push(candidate);
    }

    match detected.len() {
        0 => Err(AppError::Config("unsupported_site".into())),
        1 => Ok(detected.pop().expect("one detected backend")),
        _ => Err(AppError::Config("protocol_conflict".into())),
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

    fn newapi_status_body() -> &'static str {
        r#"{
            "success": true,
            "message": "",
            "data": {
                "version": "1.2.3",
                "system_name": "New API",
                "theme": "default",
                "register_enabled": true,
                "password_login_enabled": true
            }
        }"#
    }

    fn sub2api_body() -> &'static str {
        r#"{
            "code": 0,
            "message": "success",
            "data": {
                "site_name": "Sub2API",
                "version": "1.0.0",
                "api_base_url": "",
                "registration_enabled": true,
                "promo_code_enabled": false,
                "invitation_code_enabled": false
            }
        }"#
    }

    #[test]
    fn backend_kind_serializes_to_stable_protocol_names() {
        assert_eq!(
            serde_json::to_string(&BackendKind::Sub2Api).unwrap(),
            r#""sub2api""#
        );
        assert_eq!(
            serde_json::to_string(&BackendKind::NewApi).unwrap(),
            r#""newapi""#
        );
    }

    #[test]
    fn probe_candidates_include_sub2api_and_newapi_status() {
        assert_eq!(
            PROBE_CANDIDATES,
            &[
                ProbeCandidate {
                    id: "sub2api",
                    path: "/api/v1/settings/public",
                },
                ProbeCandidate {
                    id: "newapi",
                    path: "/api/status",
                },
            ]
        );
    }

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
        assert!(detect_site("sub2api", newapi_status_body()).is_none());
    }

    #[test]
    fn detectors_accept_only_their_own_candidate() {
        assert!(
            detect_candidate("sub2api", sub2api_body())
                .unwrap()
                .backend_kind()
                == BackendKind::Sub2Api
        );
        assert!(
            detect_candidate("newapi", newapi_status_body())
                .unwrap()
                .backend_kind()
                == BackendKind::NewApi
        );
        assert!(detect_candidate("newapi", sub2api_body()).is_none());
        assert!(detect_candidate("sub2api", newapi_status_body()).is_none());
    }

    #[test]
    fn convergence_requires_exactly_one_backend_and_deduplicates_same_backend_hits() {
        let sub2api = ProbeResponse {
            candidate_id: "sub2api".into(),
            body: sub2api_body().into(),
        };
        let newapi = ProbeResponse {
            candidate_id: "newapi".into(),
            body: newapi_status_body().into(),
        };

        assert_eq!(
            converge_probe_responses(&[]).unwrap_err().to_string(),
            "配置错误: unsupported_site"
        );
        assert_eq!(
            converge_probe_responses(std::slice::from_ref(&sub2api))
                .unwrap()
                .backend_kind(),
            BackendKind::Sub2Api
        );
        assert_eq!(
            converge_probe_responses(&[sub2api.clone(), sub2api.clone()])
                .unwrap()
                .backend_kind(),
            BackendKind::Sub2Api
        );
        assert_eq!(
            converge_probe_responses(&[sub2api, newapi])
                .unwrap_err()
                .to_string(),
            "配置错误: protocol_conflict"
        );
    }

    #[test]
    fn convergence_can_consume_webview_raw_probe_responses() {
        let response = parse_probe_navigation(&probe_callback_url("newapi", newapi_status_body()))
            .unwrap()
            .unwrap();
        assert_eq!(
            converge_probe_responses(&[response])
                .unwrap()
                .backend_kind(),
            BackendKind::NewApi
        );
    }
}
