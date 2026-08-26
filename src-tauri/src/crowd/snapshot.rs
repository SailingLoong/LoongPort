//! 公共快照的拉取与本地缓存（模式照 `relay::transit` 的 transit-cache）。
//!
//! 快照是**公共展示数据**（k-匿名后的聚合），不是行为开关 —— 拉取不带签名校验，
//! 但保留 HTTPS + 体积上限 + 防御性解析三件套。对等门禁在命令层：
//! `crowd_get_snapshot` 在共建关闭时直接返回 `None`，锁定态连拉取都不发生。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub const SNAPSHOT_URL: &str = "https://metrics.loongport.dev/v1/snapshot";

const FETCH_TIMEOUT_SECS: u64 = 8;
/// 与 remote-config 同一个量级上限：快照是几十 KB 级的公共 JSON。
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
/// 超过这个岁数就在返回缓存的同时后台刷新。
const STALE_AFTER_SECS: i64 = 5 * 60;

/// 公共快照（与 Worker `src/types.ts` 的 `Snapshot` 同形）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: i32,
    pub generated_at: i64,
    pub sites: std::collections::BTreeMap<String, SiteStats>,
    /// TTFT 桶上边界（Worker 随快照下发；唯源在 crowd-metrics/src/bins.ts）。
    /// 旧缓存/旧 Worker 缺此键时由 [`with_bin_edges`] 用本地常量补齐 —— 前端
    /// 只认这一份，不在 TS 里再抄一版边界。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ttft_bin_edges: Vec<i64>,
}

/// 补齐快照里的桶边界（缺省时取本地常量）。边界常量本身有跨语言闸测试
/// （`crowd::bins`）与 Worker 同源。
pub fn with_bin_edges(mut snapshot: Snapshot) -> Snapshot {
    if snapshot.ttft_bin_edges.is_empty() {
        snapshot.ttft_bin_edges = crate::crowd::bins::TTFT_BIN_EDGES_MS.to_vec();
    }
    snapshot
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SiteStats {
    pub w24: Option<WindowStats>,
    pub w7: Option<WindowStats>,
    pub hours: Vec<HourSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowStats {
    pub samples: i64,
    pub sources: i64,
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p95_ms: Option<f64>,
    pub err_rate: Option<f64>,
    pub cache_hit_rate: Option<f64>,
    pub cost_usd_per_m_tok: Option<f64>,
    /// 合并后的 TTFT 直方图（展示分布用；旧快照缺此键读成空，UI 隐藏分布图）。
    #[serde(default)]
    pub ttft_bins: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HourSlot {
    pub p50_ms: Option<f64>,
    pub samples: i64,
}

fn cache_path() -> PathBuf {
    crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join("crowd-snapshot-cache.json")
}

/// 读本地缓存（无缓存/损坏都返回 `None`，调用方自行决定现拉）。
pub fn read_cached() -> Option<Snapshot> {
    std::fs::read(cache_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn is_stale(snapshot: &Snapshot) -> bool {
    chrono::Utc::now().timestamp() - snapshot.generated_at > STALE_AFTER_SECS
}

/// 拉取并落缓存。体积超限或解析失败都不写缓存（脏数据不落盘）。
pub async fn refresh_and_cache() -> Result<Snapshot, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Config(format!("构造 crowd 快照客户端失败: {e}")))?;

    let resp = client
        .get(SNAPSHOT_URL)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("crowd 快照拉取失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Config(format!(
            "crowd 快照被拒: {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Config(format!("crowd 快照读取失败: {e}")))?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(AppError::Config("crowd 快照超过体积上限".into()));
    }

    let snapshot: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Config(format!("crowd 快照解析失败: {error}")))?;
    crate::config::atomic_write(&cache_path(), &bytes)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_parses_the_worker_shape() {
        // 与 Worker 聚合输出的字段名/嵌套保持同形（camelCase）。
        let raw = r#"{
          "version": 1,
          "generatedAt": 1787724360,
          "sites": {
            "example.com": {
              "w24": {
                "samples": 120, "sources": 3,
                "ttftP50Ms": 812.5, "ttftP95Ms": 2140.0,
                "errRate": 0.008, "cacheHitRate": 0.62, "costUsdPerMTok": 1.25,
                "ttftBins": [1, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]
              },
              "w7": null,
              "hours": [
                {"p50Ms": null, "samples": 0},
                {"p50Ms": 780.0, "samples": 42}
              ]
            }
          }
        }"#;
        let snap: Snapshot = serde_json::from_str(raw).unwrap();
        assert_eq!(snap.sites.len(), 1);
        let site = &snap.sites["example.com"];
        assert_eq!(site.w24.as_ref().unwrap().sources, 3);
        assert_eq!(
            site.w24.as_ref().unwrap().ttft_bins,
            vec![1, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(site.w7.is_none());
        assert_eq!(site.hours[1].samples, 42);
    }
}
