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
// Some protocol registries include large, user-visible configuration arrays in their public
// settings response. Keep the callback bounded, but do not discard a valid detector payload
// merely because unrelated settings make the response larger than the old 64 KiB limit.
const MAX_PROBE_BODY_BYTES: usize = 64 * 1024;
// Only compact source responses up to this size. The browser fetch API materializes text before
// this check, while the cross-process callback remains capped by MAX_PROBE_BODY_BYTES.
const MAX_PROBE_COMPACT_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROBE_RESPONSE_OVERHEAD_BYTES: usize = 512;

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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ProbeResponse {
    pub candidate_id: String,
    pub body: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub body_bytes: usize,
    #[serde(default)]
    pub detector_body_bytes: usize,
    #[serde(default)]
    pub json_like: bool,
    #[serde(default)]
    pub error_kind: Option<String>,
    /// 原生请求跟随重定向后的最终落地 origin；浏览器回传不含该字段 —— 探针脚本
    /// 受 origin 守卫约束，只在与锚点同源的页面上跑。
    #[serde(default)]
    pub final_origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ProbeBatch {
    pub responses: Vec<ProbeResponse>,
}

/// 生成注入所有页面的协议无关探测脚本。
///
/// 脚本只在目标 origin 上工作；验证页、注册页、登录页都走同一份代码。它不认识任何
/// 验证产品，也不在浏览器层判断协议。每轮回传安全的响应元数据；只有 JSON-like 且大小
/// 合规的正文才交给 Rust detector，验证页正文、令牌、Cookie 和请求头都不会离开 WebView。
/// 对不超过 2 MiB 的大 JSON 响应先做协议无关投影，再把最多 64 KiB 的正文交给 Rust detector。
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
  const maxCompactSourceBytes = {MAX_PROBE_COMPACT_SOURCE_BYTES};
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

  function responseContentType(response) {{
    try {{
      const value = response.headers && response.headers.get
        ? response.headers.get('content-type')
        : null;
      if (typeof value !== 'string' || !value) return null;
      return value.replace(/[\u0000-\u001f\u007f]/g, ' ').slice(0, 128);
    }} catch (_) {{
      return null;
    }}
  }}

  function safeErrorKind(error) {{
    const name = error && typeof error.name === 'string' ? error.name : '';
    const safeName = name.replace(/[^A-Za-z0-9_.-]/g, '').slice(0, 64);
    return safeName || 'Error';
  }}

  function byteLength(value) {{
    return new TextEncoder().encode(value).length;
  }}

  function setProjectedPath(target, path, value) {{
    const segments = path.split('.').filter(Boolean);
    if (segments.length === 0) return;
    let cursor = target;
    for (let index = 0; index < segments.length - 1; index += 1) {{
      const segment = segments[index];
      if (!cursor[segment] || typeof cursor[segment] !== 'object') cursor[segment] = {{}};
      cursor = cursor[segment];
    }}
    cursor[segments[segments.length - 1]] = value;
  }}

  function readJsonPath(source, path) {{
    const segments = path.split('.').filter(Boolean);
    let cursor = source;
    for (const segment of segments) {{
      if (!cursor || typeof cursor !== 'object'
          || !Object.prototype.hasOwnProperty.call(cursor, segment)) {{
        return {{ found: false, value: null }};
      }}
      cursor = cursor[segment];
    }}
    return {{ found: true, value: cursor }};
  }}

  function detectorBody(candidate, body, bodyBytes) {{
    if (bodyBytes > maxCompactSourceBytes) return '';
    if (bodyBytes <= {MAX_PROBE_BODY_BYTES}) return body;
    try {{
      const parsed = JSON.parse(body);
      const projected = {{}};
      for (const path of candidate.detector_json_paths || []) {{
        const selected = readJsonPath(parsed, path);
        if (selected.found) setProjectedPath(projected, path, selected.value);
      }}
      const compact = JSON.stringify(projected);
      return byteLength(compact) <= {MAX_PROBE_BODY_BYTES} ? compact : '';
    }} catch (_) {{
      return '';
    }}
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
            const body = await response.text();
            const bodyBytes = byteLength(body);
            return {{ response, body, bodyBytes }};
          }})();
          const timeout = new Promise((_, reject) => {{
            timeoutId = setTimeout(() => {{
              controller.abort();
              const error = new Error('probe request timed out');
              error.name = 'ProbeTimeout';
              reject(error);
            }}, requestTimeoutMs);
          }});
          const {{ response, body, bodyBytes }} = await Promise.race([request, timeout]);
          const trimmed = body.trim();
          const jsonLike = Boolean(trimmed) && (trimmed[0] === '{{' || trimmed[0] === '[');
          const bodyForDetector = jsonLike ? detectorBody(candidate, body, bodyBytes) : '';
          responses.push({{
            candidate_id: candidate.id,
            body: bodyForDetector,
            status: Number.isInteger(response.status) ? response.status : null,
            content_type: responseContentType(response),
            body_bytes: bodyBytes,
            detector_body_bytes: byteLength(bodyForDetector),
            json_like: jsonLike,
            error_kind: null,
          }});
        }} catch (error) {{
          responses.push({{
            candidate_id: candidate.id,
            body: '',
            status: null,
            content_type: null,
            body_bytes: 0,
            detector_body_bytes: 0,
            json_like: false,
            error_kind: safeErrorKind(error),
          }});
        }} finally {{
          if (timeoutId !== undefined) clearTimeout(timeoutId);
        }}
      }}

      const batch = JSON.stringify(responses);
      if (batch === previousBatch) return;
      previousBatch = batch;
      window.location.href = callbackScheme + '://response?d=' + b64url(batch);
    }} finally {{
      probeInFlight = false;
    }}
  }}

  function startProbing() {{
    void probe();
    setInterval(() => void probe(), 1500);
  }}

  // A custom-scheme callback before the first remote document commits can leave WebKit on its
  // blank initial document even when Rust rejects that navigation. Start only after DOM readiness.
  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', startProbing, {{ once: true }});
  }} else {{
    startProbing();
  }}
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
            || responses.iter().any(|response| {
                response.candidate_id.len() > 64
                    || response.body.len() > MAX_PROBE_BODY_BYTES
                    || response
                        .content_type
                        .as_deref()
                        .is_some_and(|value| value.len() > 128)
                    || response
                        .error_kind
                        .as_deref()
                        .is_some_and(|value| value.len() > 64)
            })
        {
            return Err(AppError::Config("站点探测回传正文或元数据过大".into()));
        }
        Ok(ProbeBatch { responses })
    })())
}

const fn base64url_encoded_len(decoded_len: usize) -> usize {
    (decoded_len * 4).div_ceil(3)
}

/// 生成可安全落盘的探针批次摘要，不包含响应正文、令牌、Cookie 或请求头。
pub fn probe_batch_summary(responses: &[ProbeResponse]) -> String {
    if responses.is_empty() {
        return "no_responses".into();
    }

    responses
        .iter()
        .map(|response| {
            let candidate_id = sanitize_probe_log_value(&response.candidate_id, 64);
            if let Some(error_kind) = response.error_kind.as_deref() {
                return format!(
                    "{candidate_id}(error={})",
                    sanitize_probe_log_value(error_kind, 64)
                );
            }

            let status = response
                .status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "unknown".into());
            let content_type = response
                .content_type
                .as_deref()
                .map(|value| sanitize_probe_log_value(value, 128))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".into());
            format!(
                "{candidate_id}(status={status},type={content_type},bytes={},detector_bytes={},json={})",
                response.body_bytes, response.detector_body_bytes, response.json_like
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize_probe_log_value(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(max_chars)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    let responses = probe_candidates(site_origin).await?;
    // 与旧内联实现同一条判据：**没有任何候选完成过一次 HTTP 往返**才算传输失败
    // （连不上 / 中途断流）；只要有一个候选拿到过响应（哪怕 404 / 超大），
    // 就按「站点答了话但认不出」走 UnsupportedSite。
    let completed = responses
        .iter()
        .any(|response| response.status.is_some() && response.error_kind.is_none());
    if !completed {
        let detail = responses
            .iter()
            .find_map(|response| response.error_kind.clone())
            .unwrap_or_default();
        return Err(DiscoveryError::new(
            DiscoveryErrorKind::Transport,
            format!("连接站点失败: {detail}"),
        ));
    }

    converge_probe_responses(&responses)
}

/// 原生探测一轮，返回**逐候选**的元数据（失败候选也记录 `status` / `error_kind`，
/// `body` 为空）——`discover_site` 的收敛与 [`crate::relay::site_probe`] 的三分类
/// 都吃这一份原始数据，谁也不用重打一遍网络请求。
///
/// 每个候选的落表口径：发送失败 `status=None + error_kind`；读到 HTTP 响应但
/// 状态非 2xx `status=Some + 空 body`；2xx 且读完正文 `status + body`；正文超上限
/// （与浏览器路径同一个源大小上限）按「答了话但指纹读不了」记 `status + 空 body`
/// —— 与非 2xx 同形状，靠 status 区分，**不算 error**（站点活着，别当传输失败）。
pub(crate) async fn probe_candidates(
    site_origin: &str,
) -> Result<Vec<ProbeResponse>, DiscoveryError> {
    let client = api::build_client().map_err(|error| {
        DiscoveryError::new(
            DiscoveryErrorKind::Transport,
            format!("无法建立站点连接: {error}"),
        )
    })?;
    let mut responses = Vec::new();

    for candidate in PROBE_CANDIDATES {
        let url = format!("{site_origin}{}", candidate.path);
        let mut response = ProbeResponse {
            candidate_id: candidate.id.to_string(),
            ..ProbeResponse::default()
        };
        let sent = match client.get(&url).send().await {
            Ok(sent) => sent,
            Err(error) => {
                response.error_kind = Some(sanitize_probe_log_value(&error.to_string(), 64));
                responses.push(response);
                continue;
            }
        };
        response.status = Some(sent.status().as_u16());
        response.final_origin = Some(sent.url().origin().ascii_serialization());
        response.content_type = sent
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if !sent.status().is_success() {
            responses.push(response);
            continue;
        }

        // 上限按**压缩前的源大小**算，与浏览器那条路径一致（见 `detectorBody`）：
        // 站点把大段用户可见配置塞进公共设置里是常事（实测 bestapi.store 已 143 KiB），
        // 在 64 KiB 就丢弃会把一个完全正常的 sub2api 站误判成「协议无法识别」。
        // 真正的指纹只有几个字段，读完再投影即可。
        if sent
            .content_length()
            .is_some_and(|length| length > MAX_PROBE_COMPACT_SOURCE_BYTES as u64)
        {
            responses.push(response);
            continue;
        }

        let mut body = Vec::new();
        let mut oversized = false;
        let mut stream = sent.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) if body.len() + chunk.len() <= MAX_PROBE_COMPACT_SOURCE_BYTES => {
                    body.extend_from_slice(&chunk);
                }
                Ok(_) => {
                    oversized = true;
                    break;
                }
                Err(error) => {
                    response.error_kind = Some(sanitize_probe_log_value(&error.to_string(), 64));
                    break;
                }
            }
        }
        if response.error_kind.is_some() || oversized {
            // 读流中断（error）之外的两种「没读到指纹」都按答话处理：正文超上限
            // 时丢弃半截正文 —— 存 2 MiB 解不开的 JSON 没有意义，status 已说明一切。
            responses.push(response);
            continue;
        }
        response.body_bytes = body.len();
        response.detector_body_bytes = body.len();
        response.json_like = body
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| byte == b'{' || byte == b'[');
        response.body = String::from_utf8_lossy(&body).into_owned();
        responses.push(response);
    }

    Ok(responses)
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
        let Some(mut candidate) = detect_candidate(&response.candidate_id, &response.body) else {
            continue;
        };
        candidate.final_origin = response.final_origin.clone();
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
    let body = serde_json::to_string(responses).expect("serialize test probe responses");
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
                    detector_json_paths: api::PROBE_ADAPTER.candidate.detector_json_paths,
                },
                ProbeCandidate {
                    id: "newapi",
                    path: "/api/status",
                    bearer_token_storage_key: None,
                    detector_json_paths: newapi::PROBE_ADAPTER.candidate.detector_json_paths,
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
                detector_json_paths: &["code", "data.version"],
            },
            ProbeCandidate {
                id: "future",
                path: "/api/status",
                bearer_token_storage_key: None,
                detector_json_paths: &["success", "data.system_name"],
            },
        ];
        let script = browser_probe_script("https://relay.example", &candidates);

        assert!(script.contains("/api/v1/settings/public"));
        assert!(script.contains("/api/status"));
        assert!(script.contains("credentials: 'include'"));
        assert!(script.contains(PROBE_SCHEME));
        assert!(script.contains("window.location.origin"));
        assert!(script.contains("const responses = []"));
        assert!(script.contains("function detectorBody(candidate, body, bodyBytes)"));
        assert!(script.contains("candidate.detector_json_paths"));
        assert!(script.contains("const maxCompactSourceBytes = 2097152;"));
        assert!(script.contains("error_kind: safeErrorKind(error)"));
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
            ..ProbeResponse::default()
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
                ..ProbeResponse::default()
            },
            ProbeResponse {
                candidate_id: "newapi".into(),
                body: newapi_status_body().into(),
                ..ProbeResponse::default()
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
                ..ProbeResponse::default()
            },
            ProbeResponse {
                candidate_id: "newapi".into(),
                body,
                ..ProbeResponse::default()
            },
        ];

        let parsed = parse_probe_navigation(&probe_batch_callback_url(&expected))
            .expect("probe scheme")
            .expect("aggregate batch remains valid");

        assert_eq!(parsed.responses, expected);
    }

    #[test]
    fn probe_batch_summary_reports_safe_metadata_without_response_body() {
        let responses = [
            ProbeResponse {
                candidate_id: "sub2api".into(),
                body: "<html>secret verification page</html>".into(),
                status: Some(403),
                content_type: Some("text/html; charset=UTF-8\nforged".into()),
                body_bytes: 38,
                detector_body_bytes: 0,
                json_like: false,
                error_kind: None,
                final_origin: None,
            },
            ProbeResponse {
                candidate_id: "newapi".into(),
                error_kind: Some("ProbeTimeout\nforged".into()),
                ..ProbeResponse::default()
            },
        ];

        let summary = probe_batch_summary(&responses);

        assert_eq!(
            summary,
            "sub2api(status=403,type=text/html; charset=UTF-8 forged,bytes=38,detector_bytes=0,json=false) newapi(error=ProbeTimeout forged)"
        );
        assert!(!summary.contains("secret verification page"));
        assert!(!summary.contains('\n'));
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
            ..ProbeResponse::default()
        };
        let newapi = ProbeResponse {
            candidate_id: "newapi".into(),
            body: newapi_status_body().into(),
            ..ProbeResponse::default()
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
            ..ProbeResponse::default()
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

    /// 公共设置里塞了大段无关配置的站，原生探针也必须认得出来。
    ///
    /// 回归 bestapi.store：它的 `/api/v1/settings/public` 实测 143 KiB（用户可见的
    /// 配置数组），旧上限 64 KiB 会在读之前就丢弃这个候选 ⇒ 明明是正常 sub2api 站，
    /// 却报「无法识别该站点支持的中转协议」。浏览器那条路径一直能识别（它压缩后再判），
    /// 两条路径对同一站点得出不同结论本身就是 bug。
    #[tokio::test]
    async fn native_discovery_accepts_sites_with_large_public_settings() {
        let mut body: serde_json::Value = serde_json::from_str(sub2api_body()).expect("parse");
        // 撑到远超旧的 64 KiB 上限，但仍在源大小上限之内。
        let filler: Vec<String> = (0..4000).map(|i| format!("filler-value-{i:06}")).collect();
        body["data"]["unrelated_large_setting"] = serde_json::json!(filler);
        assert!(
            serde_json::to_string(&body).expect("serialize").len() > MAX_PROBE_BODY_BYTES,
            "样本必须真的超过旧上限，否则这条测试什么都没钉住"
        );

        let app = Router::new().route(
            "/api/v1/settings/public",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let origin = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });

        let detected = probe_site(&origin)
            .await
            .expect("large settings must still detect");

        assert_eq!(detected.backend_kind, BackendKind::Sub2Api);
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
                    // 超的是**源大小**上限 —— 这条测试钉的是「超限当场中断，不挂着等 EOF」，
                    // 阈值本身换成哪个常量不影响它要守的不变量。
                    yield Ok::<_, Infallible>(Bytes::from(
                        vec![b'x'; MAX_PROBE_COMPACT_SOURCE_BYTES + 1],
                    ));
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

    #[test]
    fn convergence_propagates_final_origin_from_the_matching_candidate() {
        let response = ProbeResponse {
            candidate_id: "sub2api".into(),
            body: sub2api_body().into(),
            final_origin: Some("https://panel.example".into()),
            ..ProbeResponse::default()
        };

        let detected = converge_probe_responses(std::slice::from_ref(&response))
            .expect("valid sub2api body must converge");

        assert_eq!(
            detected.final_origin.as_deref(),
            Some("https://panel.example")
        );
    }

    /// 裸域全路径 301 到 `www.` 的站点：原生探针跟随重定向认出协议，且必须带回
    /// **最终落地 origin** —— 导入窗靠它把入口、脚本守卫与落库行锚到页面真正
    /// 停留的 origin，否则守卫锚在请求 origin 上、回传永远不来。
    #[tokio::test]
    async fn native_probe_follows_redirect_and_reports_final_origin() {
        let panel = Router::new().route(
            "/api/v1/settings/public",
            get(|| async {
                Json(serde_json::from_str::<serde_json::Value>(sub2api_body()).unwrap())
            }),
        );
        let panel_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind panel server");
        let panel_origin = format!(
            "http://{}",
            panel_listener.local_addr().expect("panel local address")
        );
        let panel_server = tokio::spawn(async move {
            axum::serve(panel_listener, panel)
                .await
                .expect("serve panel");
        });

        let redirect_target = format!("{panel_origin}/api/v1/settings/public");
        let apex = Router::new().route(
            "/api/v1/settings/public",
            get(move || async move { axum::response::Redirect::permanent(&redirect_target) }),
        );
        let apex_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind apex server");
        let apex_origin = format!(
            "http://{}",
            apex_listener.local_addr().expect("apex local address")
        );
        let apex_server = tokio::spawn(async move {
            axum::serve(apex_listener, apex).await.expect("serve apex");
        });

        let detected = probe_site(&apex_origin)
            .await
            .expect("redirected sub2api probe must converge");

        assert_eq!(detected.backend_kind, BackendKind::Sub2Api);
        assert_eq!(
            detected.final_origin.as_deref(),
            Some(panel_origin.as_str())
        );
        apex_server.abort();
        panel_server.abort();
    }
}
