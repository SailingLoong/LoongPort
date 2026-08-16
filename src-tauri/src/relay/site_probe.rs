//! 站点探针健康层：把「这个站的协议现在通不通」变成**可持久化、可复用**的事实。
//!
//! ## 与站点来源解耦（这是本模块存在的形状约束）
//!
//! 本模块**不知道**站点从哪来：签名目录、用户自建站（自定义站点那条特性线）都只是
//! 探针名单的一个输入源。对外只给四样东西：
//!
//! - [`probe_origin`]：探一个站，三分类；
//! - [`probe_and_record`]：探一批站 + 落盘 + 供漏斗日志用的逐站记录；
//! - [`SiteProbeStore`]：结果缓存与「该不该曝光」的闸；
//! - [`ProbeVerdict`]：三分类判据。
//!
//! 「哪些站该探」的编排（目录漏斗、自定义站点）住在各自领域里，别往这里搬。
//!
//! ## 三分类：网络失败 ≠ 站点不兼容
//!
//! 原生探针（reqwest）过不了 Cloudflare 托管挑战，而那种站**导入时走浏览器兜底
//! 是能成功的**（这正是 `commands::relay` 做浏览器辅助发现的理由）。所以：
//!
//! - **Supported**：严格 detector 识别出 sub2api / newapi —— 一定可用；
//! - **NetworkBlocked**：连一次完整 HTTP 往返都没有，或所有响应都是挑战形态
//!   （403/429/503 且无 JSON）—— 是**这台机器/这个网络**的问题，**不隐藏**；
//! - **UnrecognizedPanel**：站点答了话但协议认不出（404 / 形状不匹配 / 双协议冲突）
//!   —— 大概率是不兼容面板，连续 [`HIDE_AFTER_CONSECUTIVE_PANEL_MISSES`] 轮
//!   不过才从曝光里摘掉（防单次抖动闪进闪出）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::relay::backend::BackendKind;
use crate::relay::discovery::{self, ProbeResponse};

/// 连续多少轮 UnrecognizedPanel 之后从曝光摘除。周期 6 小时 ⇒ 3 轮 ≈ 18 小时。
pub const HIDE_AFTER_CONSECUTIVE_PANEL_MISSES: u32 = 3;

/// 一轮探针的并发上限 —— 与 `leaderboard` 抓详情的 `MANAGED_DETAIL_CONCURRENCY`
/// 同量级：这是后台任务，不该为了快把用户的出口带宽打满。
const PROBE_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeVerdict {
    Supported,
    NetworkBlocked,
    UnrecognizedPanel,
}

/// 一个站这一轮探针的结论。`detail` 是逐候选摘要（`probe_batch_summary` 形状），
/// 只含状态/类型/字节数，不含正文——可安全进日志与本地缓存。
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub verdict: ProbeVerdict,
    pub backend: Option<BackendKind>,
    pub detail: String,
    pub probed_at: i64,
}

/// 探一个站（原生路径，无浏览器兜底）并三分类。
pub async fn probe_origin(site_origin: &str) -> ProbeOutcome {
    let responses = match discovery::probe_candidates(site_origin).await {
        Ok(responses) => responses,
        Err(error) => {
            // 连 HTTP 客户端都建不起来 —— 环境问题，不是站点的问题。
            return ProbeOutcome {
                verdict: ProbeVerdict::NetworkBlocked,
                backend: None,
                detail: format!("client: {error}"),
                probed_at: now_ts(),
            };
        }
    };
    classify_probe_responses(&responses)
}

/// 三分类的纯函数核心（不打网络）—— 测试直接喂 [`ProbeResponse`]。
pub(crate) fn classify_probe_responses(responses: &[ProbeResponse]) -> ProbeOutcome {
    let detail = discovery::probe_batch_summary(responses);
    let at = now_ts();
    match discovery::converge_probe_responses(responses) {
        Ok(detected) => ProbeOutcome {
            verdict: ProbeVerdict::Supported,
            backend: Some(detected.backend_kind),
            detail,
            probed_at: at,
        },
        Err(error) if error.kind == discovery::DiscoveryErrorKind::ProtocolConflict => {
            // 站点同时回两种协议 —— 站方配置坏了，归「站点侧认不出」而不是环境。
            ProbeOutcome {
                verdict: ProbeVerdict::UnrecognizedPanel,
                backend: None,
                detail: format!("{}; {detail}", error.message),
                probed_at: at,
            }
        }
        Err(_) => {
            let responded = responses.iter().any(|response| response.status.is_some());
            let challenge_only = responses
                .iter()
                .filter(|response| response.status.is_some())
                .all(|response| {
                    matches!(response.status, Some(403 | 429 | 503)) && !response.json_like
                });
            let verdict = if !responded || challenge_only {
                ProbeVerdict::NetworkBlocked
            } else {
                ProbeVerdict::UnrecognizedPanel
            };
            ProbeOutcome {
                verdict,
                backend: None,
                detail,
                probed_at: at,
            }
        }
    }
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 探针记录：漏斗日志一行的事实来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRecord {
    pub host: String,
    pub verdict: ProbeVerdict,
    pub backend: Option<BackendKind>,
    pub detail: String,
    /// 摘除闸：连续第几轮 UnrecognizedPanel（其他判据清零）。
    pub consecutive_panel_misses: u32,
    /// 这一轮过后该站是否仍在曝光集合里。
    pub exposed: bool,
}

/// 并发探一批站、把结果记入本地缓存，返回逐站记录（调用方拿去落漏斗日志）。
///
/// 失败不抛：探针是尽力而为的后台动作，单站失败本身就是一轮 NetworkBlocked
/// 记录（会把上一轮的 miss 计数清零 —— 环境故障不该累计成「摘除」）。
pub async fn probe_and_record(origins: &[String]) -> Vec<ProbeRecord> {
    use futures::StreamExt;

    let outcomes = futures::stream::iter(origins.to_vec())
        .map(|origin| async move {
            let outcome = probe_origin(&origin).await;
            (origin, outcome)
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    let mut store = SiteProbeStore::load();
    let records = outcomes
        .into_iter()
        .map(|(origin, outcome)| {
            // 名单给的是 origin（含 scheme）；缓存键与曝光闸统一用 host。
            let host = host_of(&origin);
            let entry = store.record(&host, &outcome);
            ProbeRecord {
                host,
                verdict: outcome.verdict,
                backend: outcome.backend,
                detail: outcome.detail,
                consecutive_panel_misses: entry.consecutive_panel_misses,
                exposed: entry.exposed(),
            }
        })
        .collect();
    if let Err(error) = store.save() {
        log::warn!("探针结果落盘失败（曝光闸继续用上一份缓存）: {error}");
    }
    records
}

fn host_of(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| origin.trim().to_owned())
}

/// 每站的最近一次探针结论 + 连续摘除计数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteProbeEntry {
    pub verdict: ProbeVerdict,
    pub backend: Option<BackendKind>,
    pub detail: String,
    pub probed_at: i64,
    #[serde(default)]
    pub consecutive_panel_misses: u32,
}

impl SiteProbeEntry {
    /// 摘除闸：只有「连续多轮认定不兼容面板」才摘；没探过 / 网络失败 / 识别成功都曝光。
    fn exposed(&self) -> bool {
        !(self.verdict == ProbeVerdict::UnrecognizedPanel
            && self.consecutive_panel_misses >= HIDE_AFTER_CONSECUTIVE_PANEL_MISSES)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteProbeStore {
    pub schema_version: u8,
    pub entries: BTreeMap<String, SiteProbeEntry>,
}

impl Default for SiteProbeStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            entries: BTreeMap::new(),
        }
    }
}

impl SiteProbeStore {
    pub fn load() -> Self {
        Self::load_from(&store_path()).unwrap_or_default()
    }

    fn load_from(path: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice::<SiteProbeStore>(&bytes) {
            Ok(store) if store.schema_version == 1 => Some(store),
            Ok(_) => {
                log::warn!("探针缓存 schema 不认识，按空处理");
                None
            }
            Err(error) => {
                log::warn!("探针缓存损坏，按空处理: {error}");
                None
            }
        }
    }

    fn save_to(&self, path: &std::path::Path) -> Result<(), crate::error::AppError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|source| crate::error::AppError::JsonSerialize { source })?;
        crate::config::atomic_write(path, &bytes)
    }

    pub fn save(&self) -> Result<(), crate::error::AppError> {
        self.save_to(&store_path())
    }

    /// 记一轮结果并返回更新后的条目（计数规则见 [`SiteProbeEntry::exposed`]）。
    pub fn record(&mut self, host: &str, outcome: &ProbeOutcome) -> SiteProbeEntry {
        let previous = self.entries.get(host);
        let consecutive_panel_misses = match outcome.verdict {
            ProbeVerdict::UnrecognizedPanel => previous
                .map(|entry| entry.consecutive_panel_misses.saturating_add(1))
                .unwrap_or(1),
            // 识别成功 / 网络失败都清零：网络故障不该累计成「摘除」。
            ProbeVerdict::Supported | ProbeVerdict::NetworkBlocked => 0,
        };
        let entry = SiteProbeEntry {
            verdict: outcome.verdict,
            backend: outcome.backend,
            detail: outcome.detail.clone(),
            probed_at: outcome.probed_at,
            consecutive_panel_misses,
        };
        self.entries.insert(host.to_owned(), entry.clone());
        entry
    }

    /// 曝光闸。没探过的站**不摘** —— 探针 5 秒后首轮就会补上，先到先展示。
    pub fn should_expose(&self, host: &str) -> bool {
        self.entries
            .get(host)
            .map(SiteProbeEntry::exposed)
            .unwrap_or(true)
    }
}

fn store_path() -> std::path::PathBuf {
    crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join("site-probes.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn newapi_body() -> String {
        r#"{"success":true,"message":"","data":{"version":"1.2.3","system_name":"New API","theme":"default","register_enabled":true,"password_login_enabled":true}}"#.into()
    }

    fn sub2api_body() -> String {
        r#"{"code":0,"message":"success","data":{"site_name":"Best","version":"1.0.0","api_base_url":"","registration_enabled":true,"promo_code_enabled":false,"invitation_code_enabled":false}}"#.into()
    }

    fn response(candidate: &str, body: &str, status: Option<u16>) -> ProbeResponse {
        let trimmed = body.trim();
        ProbeResponse {
            candidate_id: candidate.into(),
            body: body.to_owned(),
            status,
            content_type: Some("application/json".into()),
            body_bytes: body.len(),
            detector_body_bytes: body.len(),
            json_like: trimmed.starts_with('{') || trimmed.starts_with('['),
            error_kind: None,
        }
    }

    fn failed(candidate: &str, error_kind: &str) -> ProbeResponse {
        ProbeResponse {
            candidate_id: candidate.into(),
            error_kind: Some(error_kind.into()),
            ..ProbeResponse::default()
        }
    }

    #[test]
    fn recognized_backend_is_supported() {
        let outcome = classify_probe_responses(&[response("newapi", &newapi_body(), Some(200))]);
        assert_eq!(outcome.verdict, ProbeVerdict::Supported);
        assert_eq!(outcome.backend, Some(BackendKind::NewApi));
    }

    #[test]
    fn answered_but_unrecognized_is_a_panel_miss() {
        // 双端点 404：站点活着但不是我们认识的面板（koozhan 实测形态）。
        let responses = [
            response("sub2api", "", Some(404)),
            response("newapi", "", Some(404)),
        ];
        assert_eq!(
            classify_probe_responses(&responses).verdict,
            ProbeVerdict::UnrecognizedPanel
        );
    }

    #[test]
    fn protocol_conflict_counts_against_the_site_not_the_network() {
        // 同站同时回两种协议 —— 站方配置坏了（weekly-day 类之外的另一种「认不出」）。
        let responses = [
            response("sub2api", &sub2api_body(), Some(200)),
            response("newapi", &newapi_body(), Some(200)),
        ];
        assert_eq!(
            classify_probe_responses(&responses).verdict,
            ProbeVerdict::UnrecognizedPanel
        );
    }

    #[test]
    fn no_http_exchange_at_all_is_network_blocked() {
        let responses = [
            failed("sub2api", "ConnectError"),
            failed("newapi", "Timeout"),
        ];
        assert_eq!(
            classify_probe_responses(&responses).verdict,
            ProbeVerdict::NetworkBlocked
        );
    }

    #[test]
    fn challenge_shaped_responses_are_network_blocked() {
        // 403 + 非 JSON = Cloudflare 托管挑战页 —— 环境拦的，浏览器路径可能仍能过。
        let challenge = ProbeResponse {
            candidate_id: "newapi".into(),
            status: Some(403),
            content_type: Some("text/html".into()),
            body_bytes: 4040,
            json_like: false,
            ..ProbeResponse::default()
        };
        assert_eq!(
            classify_probe_responses(&[challenge]).verdict,
            ProbeVerdict::NetworkBlocked
        );
    }

    #[test]
    fn mixed_404_and_network_error_still_counts_as_answered() {
        // 只要有一个端点答了话（404 也是答话），就不是纯环境问题。
        let responses = [
            response("sub2api", "", Some(404)),
            failed("newapi", "Timeout"),
        ];
        assert_eq!(
            classify_probe_responses(&responses).verdict,
            ProbeVerdict::UnrecognizedPanel
        );
    }

    #[test]
    fn consecutive_misses_gate_exposure_and_other_verdicts_reset_them() {
        let mut store = SiteProbeStore::default();
        let miss = ProbeOutcome {
            verdict: ProbeVerdict::UnrecognizedPanel,
            backend: None,
            detail: "x".into(),
            probed_at: 1,
        };
        for round in 1..=HIDE_AFTER_CONSECUTIVE_PANEL_MISSES {
            let entry = store.record("koozhan.example", &miss);
            assert_eq!(entry.consecutive_panel_misses, round);
            // 只有到了阈值那一轮才摘除。
            assert_eq!(
                store.should_expose("koozhan.example"),
                round < HIDE_AFTER_CONSECUTIVE_PANEL_MISSES
            );
        }

        // 一轮网络失败就把计数清零 —— 环境故障不该累计成摘除。
        let blocked = ProbeOutcome {
            verdict: ProbeVerdict::NetworkBlocked,
            backend: None,
            detail: "x".into(),
            probed_at: 2,
        };
        let entry = store.record("koozhan.example", &blocked);
        assert_eq!(entry.consecutive_panel_misses, 0);
        assert!(store.should_expose("koozhan.example"));
    }

    #[test]
    fn never_probed_hosts_stay_exposed() {
        assert!(SiteProbeStore::default().should_expose("fresh.example"));
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("site-probes.json");
        let mut store = SiteProbeStore::default();
        store.record(
            "bestapi.store",
            &ProbeOutcome {
                verdict: ProbeVerdict::Supported,
                backend: Some(BackendKind::Sub2Api),
                detail: "ok".into(),
                probed_at: 42,
            },
        );
        store.save_to(&path).expect("save");
        let loaded = SiteProbeStore::load_from(&path).expect("load");
        assert_eq!(
            loaded.entries.get("bestapi.store").map(|e| e.verdict),
            Some(ProbeVerdict::Supported)
        );
        assert!(loaded.should_expose("bestapi.store"));
    }
}
