//! 协议无关的浏览器辅助站点发现。
//!
//! WebView 只负责复用用户刚刚完成网页验证的同源会话，并抓取注册表中的候选端点；
//! **不在浏览器层猜站点类型**。原始响应回到 Rust 后，才由各协议 detector 严格判定。
//! 将来接 new-api 时，只需增加候选与 detector，不改“打开网页 → 用户验证”的共用流程。

use crate::error::AppError;
use crate::relay::api;
use crate::relay::backend::ProbeAdapter;
pub use crate::relay::backend::{BackendKind, DetectedSite, ProbeCandidate};
use crate::relay::newapi;
use base64::Engine;
use futures::StreamExt;
use std::fmt;

pub const PROBE_SCHEME: &str = "loongport-probe";
const MAX_PROBE_BODY_BYTES: usize = 64 * 1024;
const MAX_PROBE_RESPONSE_OVERHEAD_BYTES: usize = 256;

pub const PROBE_CANDIDATES: &[ProbeCandidate] = &[
    api::PROBE_ADAPTER.candidate,
    newapi::PROBE_ADAPTER.candidate,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryErrorKind {
    UnsupportedSite,
    ProtocolConflict,
    Transport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryError {
    pub kind: DiscoveryErrorKind,
    pub message: String,
}

impl DiscoveryError {
    fn new(kind: DiscoveryErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DiscoveryError {}

// JSON can escape a body byte as a six-byte `\\u00XX` sequence. This bound permits every
// registered candidate's maximum body plus its object metadata, while keeping callback input
// bounded before base64 decoding.
const MAX_PROBE_BATCH_BYTES: usize =
    2 + PROBE_CANDIDATES.len() * (MAX_PROBE_BODY_BYTES * 6 + MAX_PROBE_RESPONSE_OVERHEAD_BYTES);

const PROBE_ADAPTERS: &[ProbeAdapter] = &[api::PROBE_ADAPTER, newapi::PROBE_ADAPTER];

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ProbeResponse {
    pub candidate_id: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ProbeBatch {
    pub responses: Vec<ProbeResponse>,
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
  const requestTimeoutMs = 5000;
  let previousBatch = '';
  let probeInFlight = false;

  function b64url(value) {{
    const bytes = new TextEncoder().encode(value);
    let binary = '';
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
  }}

  function headersFor(candidate) {{
    const headers = {{ Accept: 'application/json' }};
    if (!candidate.bearer_token_storage_key) return headers;
    try {{
      const token = localStorage.getItem(candidate.bearer_token_storage_key);
      if (token) headers.Authorization = 'Bearer ' + token;
    }} catch (_) {{
      // Some verification pages disable storage. Cookie-only probing still remains available.
    }}
    return headers;
  }}

  async function probe() {{
    if (window.location.origin !== expectedOrigin) return;
    if (probeInFlight) return;
    probeInFlight = true;

    try {{
      const responses = [];
      for (const candidate of candidates) {{
        const controller = new AbortController();
        let timeoutId;
        try {{
          const request = (async () => {{
            const response = await fetch(candidate.path, {{
              credentials: 'include',
              cache: 'no-store',
              headers: headersFor(candidate),
              signal: controller.signal,
            }});
            return response.text();
          }})();
          const timeout = new Promise((_, reject) => {{
            timeoutId = setTimeout(() => {{
              controller.abort();
              reject(new Error('probe request timed out'));
            }}, requestTimeoutMs);
          }});
          const body = await Promise.race([request, timeout]);
          const trimmed = body.trim();
          if (!trimmed || (trimmed[0] !== '{{' && trimmed[0] !== '[')) continue;
          if (new TextEncoder().encode(body).length > {MAX_PROBE_BODY_BYTES}) continue;
          responses.push({{ candidate_id: candidate.id, body }});
        }} catch (_) {{
          // 页面仍可能处于验证或跳转阶段；下一轮继续，不在浏览器层解释失败原因。
        }} finally {{
          if (timeoutId !== undefined) clearTimeout(timeoutId);
        }}
      }}

      if (!responses.length) return;
      const batch = JSON.stringify(responses);
      if (batch === previousBatch) return;
      previousBatch = batch;
      window.location.href = callbackScheme + '://response?d=' + b64url(batch);
    }} finally {{
      probeInFlight = false;
    }}
  }}

  void probe();
  setInterval(() => void probe(), 1500);
}})();
"#
    )
}

/// 解析 WebView 的探测回传导航。`None` 表示普通网页导航，应继续放行。
pub fn parse_probe_navigation(url: &url::Url) -> Option<Result<ProbeBatch, AppError>> {
    if url.scheme() != PROBE_SCHEME {
        return None;
    }

    Some((|| {
        let encoded = url
            .query_pairs()
            .find(|(key, _)| key == "d")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| AppError::Config("站点探测回传缺少响应正文".into()))?;
        if encoded.len() > base64url_encoded_len(MAX_PROBE_BATCH_BYTES) {
            return Err(AppError::Config("站点探测回传正文过大".into()));
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| AppError::Config(format!("站点探测回传无法解码: {e}")))?;
        if bytes.len() > MAX_PROBE_BATCH_BYTES {
            return Err(AppError::Config("站点探测回传正文过大".into()));
        }
        let responses = serde_json::from_slice::<Vec<ProbeResponse>>(&bytes)
            .map_err(|e| AppError::Config(format!("站点探测回传不是候选响应批次: {e}")))?;
        if responses.len() > PROBE_CANDIDATES.len()
            || responses
                .iter()
                .any(|response| response.body.len() > MAX_PROBE_BODY_BYTES)
        {
            return Err(AppError::Config("站点探测回传正文过大".into()));
        }
        Ok(ProbeBatch { responses })
    })())
}

const fn base64url_encoded_len(decoded_len: usize) -> usize {
    (decoded_len * 4).div_ceil(3)
}

/// 用原生 HTTP 跑候选注册表的 fast path。
///
/// 任何传输失败、非成功状态或 detector 不匹配都只意味着“这个候选没有识别出来”；
/// 不在这里把某次失败宣判成站点类型。全部候选都不匹配时，调用方可切到可见 WebView。
pub async fn probe_site(site_origin: &str) -> Result<DetectedSite, DiscoveryError> {
    discover_site(site_origin).await
}

/// 原生 HTTP 探测所有候选，再复用 WebView 原始回传使用的同一收敛规则。
pub async fn discover_site(site_origin: &str) -> Result<DetectedSite, DiscoveryError> {
    let client = api::build_client().map_err(|error| {
        DiscoveryError::new(
            DiscoveryErrorKind::Transport,
            format!("无法建立站点连接: {error}"),
        )
    })?;
    let mut responses = Vec::new();
    let mut completed_candidate = false;
    let mut last_transport_error = None;

    for candidate in PROBE_CANDIDATES {
        let url = format!("{site_origin}{}", candidate.path);
        let response = match client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                last_transport_error = Some(error.to_string());
                continue;
            }
        };
        if !response.status().is_success() {
            completed_candidate = true;
            continue;
        }

        if response
            .content_length()
            .is_some_and(|length| length > MAX_PROBE_BODY_BYTES as u64)
        {
            completed_candidate = true;
            continue;
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        let mut body_failed = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) if body.len() + chunk.len() <= MAX_PROBE_BODY_BYTES => {
                    body.extend_from_slice(&chunk);
                }
                Ok(_) => {
                    completed_candidate = true;
                    body_failed = true;
                    break;
                }
                Err(error) => {
                    last_transport_error = Some(error.to_string());
                    body_failed = true;
                    break;
                }
            }
        }
        if body_failed {
            continue;
        }
        completed_candidate = true;
        responses.push(ProbeResponse {
            candidate_id: candidate.id.to_string(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }

    if responses.is_empty() && !completed_candidate {
        return Err(DiscoveryError::new(
            DiscoveryErrorKind::Transport,
            last_transport_error
                .map(|error| format!("连接站点失败: {error}"))
                .unwrap_or_else(|| "连接站点失败".into()),
        ));
    }

    converge_probe_responses(&responses)
}

/// 在 Rust 侧按候选 id 分派严格 detector。
///
/// 未知候选或形状不匹配都返回 `None`。浏览器层永远不根据端点存在、HTTP 状态或通用字段
/// 直接认定协议，因此 new-api 的响应不会被 sub2api detector 误收。
pub fn detect_candidate(candidate_id: &str, body: &str) -> Option<DetectedSite> {
    let adapter = PROBE_ADAPTERS
        .iter()
        .find(|adapter| adapter.candidate.id == candidate_id)?;
    (adapter.detect)(body)
}

/// 旧命令层的兼容入口：只暴露现有 sub2api 结果，不把 newapi 伪装成 sub2api。
#[allow(dead_code)]
pub fn detect_site(candidate_id: &str, body: &str) -> Option<DetectedSite> {
    let detected = detect_candidate(candidate_id, body)?;
    (detected.backend_kind == BackendKind::Sub2Api).then_some(detected)
}

/// 汇总所有原始候选响应，去重后严格收敛为一个协议结果。
pub fn converge_probe_responses(
    responses: &[ProbeResponse],
) -> Result<DetectedSite, DiscoveryError> {
    let mut detected = Vec::new();
    for response in responses {
        let Some(candidate) = detect_candidate(&response.candidate_id, &response.body) else {
            continue;
        };
        if detected
            .iter()
            .any(|item: &DetectedSite| item.backend_kind == candidate.backend_kind)
        {
            continue;
        }
        detected.push(candidate);
    }

    match detected.len() {
        0 => Err(DiscoveryError::new(
            DiscoveryErrorKind::UnsupportedSite,
            "无法识别该站点支持的中转协议",
        )),
        1 => Ok(detected.pop().expect("one detected backend")),
        _ => Err(DiscoveryError::new(
            DiscoveryErrorKind::ProtocolConflict,
            "站点同时返回多种中转协议，无法安全选择",
        )),
    }
}

#[cfg(test)]
fn probe_batch_callback_url(responses: &[ProbeResponse]) -> url::Url {
    let body = serde_json::json!(responses
        .iter()
        .map(|response| serde_json::json!({
            "candidate_id": response.candidate_id,
            "body": response.body,
        }))
        .collect::<Vec<_>>())
    .to_string();
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body);
    url::Url::parse(&format!("{PROBE_SCHEME}://response?d={encoded}"))
        .expect("test batch callback URL")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};

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
                    bearer_token_storage_key: Some("auth_token"),
                },
                ProbeCandidate {
                    id: "newapi",
                    path: "/api/status",
                    bearer_token_storage_key: None,
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
                bearer_token_storage_key: Some("auth_token"),
            },
            ProbeCandidate {
                id: "future",
                path: "/api/status",
                bearer_token_storage_key: None,
            },
        ];
        let script = browser_probe_script("https://relay.example", &candidates);

        assert!(script.contains("/api/v1/settings/public"));
        assert!(script.contains("/api/status"));
        assert!(script.contains("credentials: 'include'"));
        assert!(script.contains(PROBE_SCHEME));
        assert!(script.contains("window.location.origin"));
        assert!(script.contains("const responses = []"));
        assert!(script.contains("responses.push({ candidate_id: candidate.id, body })"));
        assert!(script.contains("const batch = JSON.stringify(responses)"));
        assert!(script.contains("b64url(batch)"));
        assert_eq!(
            script.matches("window.location.href =").count(),
            1,
            "each probe round must navigate once with the complete candidate batch"
        );
        assert!(!script.contains("Cloudflare"));
        assert!(!script.contains("cf_clearance"));
        assert!(!script.contains("403"));
    }

    #[test]
    fn probe_navigation_round_trips_candidate_and_raw_json() {
        let body = r#"{"code":0,"data":{"version":"1"}}"#;
        let url = probe_batch_callback_url(&[ProbeResponse {
            candidate_id: "sub2api".into(),
            body: body.into(),
        }]);
        let parsed = parse_probe_navigation(&url)
            .expect("probe scheme")
            .expect("valid callback");
        assert_eq!(parsed.responses.len(), 1);
        assert_eq!(parsed.responses[0].candidate_id, "sub2api");
        assert_eq!(parsed.responses[0].body, body);
    }

    #[test]
    fn batch_probe_navigation_round_trips_multiple_candidate_bodies() {
        let expected = vec![
            ProbeResponse {
                candidate_id: "sub2api".into(),
                body: sub2api_body().into(),
            },
            ProbeResponse {
                candidate_id: "newapi".into(),
                body: newapi_status_body().into(),
            },
        ];
        let parsed = parse_probe_navigation(&probe_batch_callback_url(&expected))
            .expect("probe scheme")
            .expect("valid batch callback");

        assert_eq!(parsed.responses, expected);
    }

    #[test]
    fn batch_probe_navigation_accepts_two_valid_medium_sized_bodies() {
        let body = format!(r#"{{"payload":"{}"}}"#, "x".repeat(40 * 1024));
        let expected = vec![
            ProbeResponse {
                candidate_id: "sub2api".into(),
                body: body.clone(),
            },
            ProbeResponse {
                candidate_id: "newapi".into(),
                body,
            },
        ];

        let parsed = parse_probe_navigation(&probe_batch_callback_url(&expected))
            .expect("probe scheme")
            .expect("aggregate batch remains valid");

        assert_eq!(parsed.responses, expected);
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
                .backend_kind
                == BackendKind::Sub2Api
        );
        assert!(
            detect_candidate("newapi", newapi_status_body())
                .unwrap()
                .backend_kind
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
            converge_probe_responses(&[]).unwrap_err().kind,
            DiscoveryErrorKind::UnsupportedSite
        );
        assert_eq!(
            converge_probe_responses(std::slice::from_ref(&sub2api))
                .unwrap()
                .backend_kind,
            BackendKind::Sub2Api
        );
        assert_eq!(
            converge_probe_responses(&[sub2api.clone(), sub2api.clone()])
                .unwrap()
                .backend_kind,
            BackendKind::Sub2Api
        );
        assert_eq!(
            converge_probe_responses(&[sub2api, newapi])
                .unwrap_err()
                .kind,
            DiscoveryErrorKind::ProtocolConflict
        );
    }

    #[test]
    fn convergence_can_consume_webview_raw_probe_responses() {
        let batch = parse_probe_navigation(&probe_batch_callback_url(&[ProbeResponse {
            candidate_id: "newapi".into(),
            body: newapi_status_body().into(),
        }]))
        .unwrap()
        .unwrap();
        assert_eq!(
            converge_probe_responses(&batch.responses)
                .unwrap()
                .backend_kind,
            BackendKind::NewApi
        );
    }

    #[tokio::test]
    async fn native_probe_site_accepts_detected_newapi() {
        let app = Router::new().route(
            "/api/status",
            get(|| async {
                Json(serde_json::from_str::<serde_json::Value>(newapi_status_body()).unwrap())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let origin = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });

        let detected = probe_site(&origin).await.expect("accept newapi probe");

        assert_eq!(detected.backend_kind, BackendKind::NewApi);
        server.abort();
    }

    #[tokio::test]
    async fn native_discovery_rejects_oversized_stream_without_waiting_for_eof() {
        use axum::body::Body;
        use axum::response::Response;
        use bytes::Bytes;
        use std::convert::Infallible;

        let app = Router::new().route(
            "/api/v1/settings/public",
            get(|| async {
                let stream = async_stream::stream! {
                    yield Ok::<_, Infallible>(Bytes::from(vec![b'x'; MAX_PROBE_BODY_BYTES + 1]));
                    std::future::pending::<()>().await;
                };
                Response::new(Body::from_stream(stream))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind oversized discovery server");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let error =
            tokio::time::timeout(std::time::Duration::from_millis(250), probe_site(&origin))
                .await
                .expect("oversized body is rejected before the stream ends")
                .expect_err("oversized body cannot identify a backend");

        assert_eq!(error.kind, DiscoveryErrorKind::UnsupportedSite);
        server.abort();
    }

    #[test]
    fn tracked_browser_probe_script_matches_the_current_generator() {
        assert_eq!(
            include_str!("../../../tests/fixtures/browser-probe-script.txt"),
            browser_probe_script("https://relay.example", PROBE_CANDIDATES),
            "the mandatory Vitest fixture must stay byte-for-byte current",
        );
    }
}
