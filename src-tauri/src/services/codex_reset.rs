//! Codex 全局重置预告（挂在官方订阅额度展示下方）。
//!
//! OpenAI 会对付费订阅做不定期「善意全局重置」，由 Codex 负责人在 X 上公告
//! ——没有固定时刻，官方也不提供接口。数据走**网站的同源代理**
//! `https://loongport.dev/api/codex-reset`：拉取社区源（codex-reset.com）、
//! 归一与「预计落地时间」解析都在那一处唯源实现（网站卡片与 App 共用，
//! 上游换了只改网站）。本模块只做：按需 GET + 1 小时缓存 + 网络失败回退
//! 旧缓存（宁可旧，不报错打断额度面板）。
//!
//! 隐私：只读公开社区数据，不带任何本机信息。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 网站代理端点（与 relay/remote_config.rs 的 CONFIG_URL 同款「app 信自家
/// 服务」的依赖形态；Rust 侧 reqwest 非浏览器，无 CORS 顾虑）。
const FEED_URL: &str = "https://loongport.dev/api/codex-reset";
const CACHE_TTL: Duration = Duration::from_secs(3600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 归一化形状（与网站投影逐字段对齐，camelCase）。所有字段可缺省——
/// 拿不到就少显示，`Option` 全给前端自己取舍。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexResetFeed {
    /// 上次已验证重置（epoch 秒）+ 原帖链接
    pub last_verified_at: Option<i64>,
    pub last_verified_url: Option<String>,
    /// 最新一条重置公告（epoch 秒 + 原帖链接 + 原文摘要）
    pub announcement_at: Option<i64>,
    pub announcement_url: Option<String>,
    pub announcement_summary: Option<String>,
    /// 公告文本窄解析出的预计落地时间（epoch 秒）；解析不出为 None
    pub landing_at: Option<i64>,
    pub fetched_at: i64,
}

static CACHE: OnceLock<Mutex<Option<(Instant, CodexResetFeed)>>> = OnceLock::new();

fn cache_cell() -> &'static Mutex<Option<(Instant, CodexResetFeed)>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 读缓存；新鲜直接用，过期/没有则拉一次。网络失败时回退旧缓存（若有），
/// 旧缓存也没有才 Err——面板对 Err 的处理是整块不渲染（宁缺毋认）。
pub async fn get_feed() -> Result<CodexResetFeed, String> {
    {
        let cell = cache_cell().lock().expect("codex reset cache poisoned");
        if let Some((at, feed)) = cell.as_ref() {
            if at.elapsed() < CACHE_TTL {
                return Ok(feed.clone());
            }
        }
    }
    match fetch_feed().await {
        Ok(feed) => {
            *cache_cell().lock().expect("codex reset cache poisoned") =
                Some((Instant::now(), feed.clone()));
            Ok(feed)
        }
        Err(error) => {
            let cell = cache_cell().lock().expect("codex reset cache poisoned");
            if let Some((_, feed)) = cell.as_ref() {
                log::warn!("codex-reset feed 拉取失败（回退旧缓存）: {error}");
                return Ok(feed.clone());
            }
            Err(error)
        }
    }
}

async fn fetch_feed() -> Result<CodexResetFeed, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(FEED_URL)
        .header("User-Agent", "LoongPort")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("feed status {}", resp.status()));
    }
    resp.json::<CodexResetFeed>()
        .await
        .map_err(|e| format!("feed 形状不认识: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约闸：网站投影的 camelCase 字段必须逐个对得上（跨语言唯源握手）。
    /// 解析逻辑的测试唯源在 website 仓 `src/lib/codexReset.test.ts`。
    #[test]
    fn website_projection_deserializes_field_by_field() {
        let raw = serde_json::json!({
            "lastVerifiedAt": 1787625586,
            "lastVerifiedUrl": "https://x.com/s/1",
            "announcementAt": 1787796297,
            "announcementUrl": "https://x.com/s/2",
            "announcementSummary": "Lands around 6pm PST today",
            "landingAt": 1787803200,
            "fetchedAt": 1787796300
        });
        let feed: CodexResetFeed = serde_json::from_value(raw).unwrap();
        assert_eq!(feed.last_verified_at, Some(1787625586));
        assert_eq!(
            feed.announcement_summary.as_deref(),
            Some("Lands around 6pm PST today")
        );
        assert_eq!(feed.landing_at, Some(1787803200));
        assert_eq!(feed.fetched_at, 1787796300);
    }

    /// 极简/缺字段响应不炸：serde `default` 全空，前端按可缺省渲染。
    #[test]
    fn sparse_projection_deserializes_to_defaults() {
        let feed: CodexResetFeed = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(feed, CodexResetFeed::default());
    }
}
