//! sub2api 站点的 ai-transit.v1 一手数据：`/.well-known/ai-transit.json` 发现 +
//! snapshot 快照解析，产出广场行要展示的**价格与可用性摘要**。
//!
//! ## 为什么接协议而不接聚合站（PriceAI / Oken）
//!
//! PriceAI 的中转站榜单数据源就是这套站方公开协议（它页面上「站方公开」一列
//! 链到的 `/public/transit` 即同一份）。站方才是价格与可用性事实的 owner
//! （尺子 1.4）：经聚合站转一手既慢又可能被它的口径改写，聚合站自身也
//! 没有承诺任何数据接口。New API 系站点没有这套协议，广场行只是不显示
//! 这两个徽章，不影响展示与导入。
//!
//! ## 快照地址为什么只强制 HTTPS、不强制同源
//!
//! 实测存在合法的跨子域部署：站点 well-known 部署在裸域、snapshot 落在
//! `api.` 子域（见收录调研）。同源校验会把真实站点误杀。这里的数据是
//! **站方自报的公开定价/可用性**，只用于展示徽章、不进任何信任决策
//! （导入闸走探针与签名配置，与本模块无关），所以 HTTPS + 体积闸 +
//! 防御式解析已经够了。
//!
//! ## 缓存语义：读不做新鲜度闸
//!
//! 刷新由 `maintenance` 的 veridrop-directory 周期（6 小时）驱动；读取路径
//! （广场列表）只合并已有摘要。数据最多「旧一个周期」，比「因为刚抓取
//! 失败就整个消失」好——徽章闪没闪现比数字旧几小时更伤信任。失败的站
//! 保留上一轮的旧值，不擦除。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 发现端点的固定路径（RFC 8615 well-known 协议惯例）。
const WELL_KNOWN_PATH: &str = "/.well-known/ai-transit.json";

/// 快照与发现端点的公共体积上限。与 `leaderboard::MAX_PAGE_BYTES` 同量级：
/// 一份真实快照几十 KB（含逐模型价格表），2 MiB 宽裕得离谱。
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

const FETCH_TIMEOUT_SECS: u64 = 12;

/// 与探针（`site_probe::PROBE_CONCURRENCY`）、榜单详情补拉同量级：
/// 后台任务不该为了快把用户的出口带宽打满。
const REFRESH_CONCURRENCY: usize = 4;

const CACHE_SCHEMA_VERSION: u8 = 1;

/// 唯一认的协议版本。站点未来出 v2 时这里不会误读——版本不匹配按
/// 「该站没有 transit 数据」处理，而不是拿错口径的数字展示给用户。
const SCHEMA_VERSION: &str = "ai-transit.v1";

/// 广场行展示用的摘要。两个数字都取**最保守值**（最低倍率 / 最低分组
/// 可用性）：用户按徽章做的是「这家最便宜多少、最差稳到什么程度」的
/// 预期，用均值会把最差分组藏起来。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitSummary {
    /// 各分组综合倍率的最小值（`composite_multiplier`，老版 sub2api 只有
    /// `rate_multiplier`，兜底取它）。`None` = 快照里没有可用的倍率字段。
    pub min_multiplier: Option<f64>,
    /// 各分组近 7 日可用性的最小值（0-100）。
    pub min_availability_7d: Option<f64>,
    /// 快照的 `generated_at`（站方口径的数据时间，不是我们抓取的时间）。
    pub synced_at: i64,
}

/// 缓存文件结构。`Eq` 只是 BTreeMap 键序稳定的附带要求，摘要里的 f64
/// 不参与任何 HashMap 语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransitCache {
    schema_version: u8,
    /// host（归一后）→ 摘要。读取方是 [`summaries`]，写入方只有周期刷新。
    entries: BTreeMap<String, TransitSummary>,
}

impl Default for TransitCache {
    fn default() -> Self {
        Self {
            schema_version: CACHE_SCHEMA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

/// well-known 发现文档。只取我们消费的两个字段，其余（homepage_url 等）
/// 靠 serde 忽略未知字段跳过。
#[derive(Debug, Deserialize)]
struct WellKnown {
    schema_version: String,
    snapshot_url: String,
}

/// 快照的防御式投影：所有字段可缺、数组默认空——站方升级/降级 sub2api
/// 都不该让解析失败（失败的表现就是「没有徽章」，静默且无害）。
#[derive(Debug, Default, Deserialize)]
struct TransitSnapshot {
    #[serde(default)]
    groups: Vec<TransitGroup>,
    #[serde(default)]
    monitoring: Vec<TransitMonitor>,
    #[serde(default)]
    generated_at: String,
}

#[derive(Debug, Default, Deserialize)]
struct TransitGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    composite_multiplier: Option<f64>,
    #[serde(default)]
    rate_multiplier: Option<f64>,
    /// 逐模型条目（能力查询消费；摘要不需要它，空数组照常出摘要）。
    #[serde(default)]
    models: Vec<TransitGroupModel>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitGroupModel {
    /// 标准模型名（官方目录口径）。
    #[serde(default)]
    standard_model: Option<String>,
    /// 站点自己的模型 id —— `/v1/models` 返回的就是它。
    #[serde(default)]
    raw_model: Option<String>,
    #[serde(default)]
    supported_protocols: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitMonitor {
    #[serde(default)]
    availability_7d: Option<f64>,
}

/// 校验 well-known 发现文档，通过则返回可抓取的 snapshot URL。
///
/// 两条硬规则：协议版本精确匹配；snapshot_url 必须 HTTPS（明文会同时
/// 暴露「谁在用 LoongPort」与可被链路注入的价格数字）。跨子域放行，
/// 理由见模块文档。
fn validate_well_known(well_known: &WellKnown) -> Result<url::Url, AppError> {
    if well_known.schema_version != SCHEMA_VERSION {
        return Err(AppError::Config(format!(
            "ai-transit 协议版本不认：{}（只认 {SCHEMA_VERSION}）",
            well_known.schema_version
        )));
    }
    let snapshot_url = url::Url::parse(&well_known.snapshot_url)
        .map_err(|error| AppError::Config(format!("snapshot_url 不合法: {error}")))?;
    if snapshot_url.scheme() != "https" {
        return Err(AppError::Config("snapshot_url 必须是 HTTPS".into()));
    }
    Ok(snapshot_url)
}

/// 从快照算摘要。纯函数，测试直接喂 [`TransitSnapshot`] 的 JSON。
fn summarize(snapshot: &TransitSnapshot) -> TransitSummary {
    // 倍率取 composite，老版 sub2api 没有 composite 就退 rate（两者实测
    // 同时存在时值一致；0 与负数是脏数据，过滤掉）。
    let min_multiplier = snapshot
        .groups
        .iter()
        .filter_map(|group| group.composite_multiplier.or(group.rate_multiplier))
        .filter(|multiplier| *multiplier > 0.0 && multiplier.is_finite())
        .fold(None::<f64>, |acc, value| {
            Some(match acc {
                Some(current) => current.min(value),
                None => value,
            })
        });
    let min_availability_7d = snapshot
        .monitoring
        .iter()
        .filter_map(|monitor| monitor.availability_7d)
        .filter(|availability| (0.0..=100.0).contains(availability))
        .fold(None::<f64>, |acc, value| {
            Some(match acc {
                Some(current) => current.min(value),
                None => value,
            })
        });
    TransitSummary {
        min_multiplier,
        min_availability_7d,
        synced_at: parse_timestamp(&snapshot.generated_at),
    }
}

/// `generated_at`（RFC 3339）→ Unix 秒。解不出给 0（前端按「无时间」处理），
/// 不值得为一个展示用时间戳引入失败路径。
fn parse_timestamp(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp())
        .unwrap_or(0)
}

fn well_known_url(host: &str) -> String {
    format!("https://{host}{WELL_KNOWN_PATH}")
}

/// 抓一个站的 transit 摘要：well-known 发现 → 校验 → 抓快照 → 摘要。
///
/// `well_known_url` 参数化（而不是内部拼 `https://{host}`）是为了让本地
/// 测试服务（http）能驱动整条链路——生产路径的 https 前缀由
/// [`refresh_for_hosts`] 钉死。任何一步失败都返回 `Err`，由
/// [`apply_results`] 决定保留旧值。
async fn fetch_summary(
    client: &reqwest::Client,
    well_known_url: &str,
) -> Result<TransitSummary, AppError> {
    let well_known: WellKnown = fetch_json(client, well_known_url).await?;
    let snapshot_url = validate_well_known(&well_known)?;
    let snapshot: TransitSnapshot = fetch_json(client, snapshot_url.as_str()).await?;
    Ok(summarize(&snapshot))
}

/// 流式抓取 + 体积闸 + 一次性 JSON 解析。
///
/// 与 `remote_config::fetch_bytes` 同一条纪律：**边读边判**，不能
/// `.bytes().await` 之后再看长度——那样上限形同虚设。
async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, AppError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| AppError::Config(format!("ai-transit 请求失败: {error}")))?;
    if !response.status().is_success() {
        return Err(AppError::Config(format!(
            "ai-transit 端点被拒: {}",
            response.status()
        )));
    }
    if let Some(length) = response.content_length() {
        if length > MAX_SNAPSHOT_BYTES as u64 {
            return Err(AppError::Config(format!(
                "ai-transit 响应声明体积就超限（{length} 字节）"
            )));
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|error| AppError::Config(format!("ai-transit 读取失败: {error}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(AppError::Config("ai-transit 响应超过体积上限".into()));
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Config(format!("ai-transit 解析失败: {error}")))
}

fn cache_path() -> std::path::PathBuf {
    crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join("transit-cache.json")
}

fn read_cache() -> TransitCache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(|cache: &TransitCache| cache.schema_version == CACHE_SCHEMA_VERSION)
        .unwrap_or_default()
}

fn write_cache(cache: &TransitCache) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(cache).map_err(|source| AppError::JsonSerialize { source })?;
    crate::config::atomic_write(&cache_path(), &bytes)
}

/// 读全部摘要（host → summary）。读取方在拼广场列表时一次载入做 join。
pub fn summaries() -> BTreeMap<String, TransitSummary> {
    read_cache().entries
}

/// 站点的**模型协议能力图**：分组名 →（模型名 → 该站声明的支持协议集合）。
///
/// 消费方是模型验证的协议预筛（`model_verification::target`）：档位记着
/// 分组名（`ProviderMeta::loongport_group`），用它在这里查「这个分组下的
/// 这个模型支不支持当前验证协议」。**分组只有名字可 join**——ai-transit
/// 快照的分组不带 id。
#[derive(Debug, Default, Clone)]
pub struct ModelProtocolCapabilities {
    groups: BTreeMap<String, BTreeMap<String, std::collections::BTreeSet<String>>>,
}

impl ModelProtocolCapabilities {
    /// 「该分组 × 该模型」的协议清单。
    ///
    /// `None` = 快照**没有正向覆盖**（分组不在快照里，或模型不在该分组的
    /// 清单里）——按 Unknown 处理、照常显示，绝不能当作「不支持」。
    /// 站点快照的分组覆盖是部分的（实测有站只发布部分平台分组），把
    /// 「没提到」读成「不支持」就是误杀。
    pub fn protocols_for(
        &self,
        group: &str,
        model: &str,
    ) -> Option<&std::collections::BTreeSet<String>> {
        self.groups.get(group)?.get(model)
    }
}

/// 从快照建能力图。纯函数——分组下每个模型以 standard 与 raw 两个名字
/// 入图（`/v1/models` 返回站点自己的 id，两种形态都可能是它）；同名条目
/// 的协议取并集（任何一条清单声明支持即算支持，正向口径）。
fn build_model_capabilities(snapshot: &TransitSnapshot) -> ModelProtocolCapabilities {
    let mut groups: BTreeMap<String, BTreeMap<String, std::collections::BTreeSet<String>>> =
        BTreeMap::new();
    for group in &snapshot.groups {
        if group.name.is_empty() {
            continue;
        }
        let entry = groups.entry(group.name.clone()).or_default();
        for model in &group.models {
            let protocols: std::collections::BTreeSet<String> = model
                .supported_protocols
                .iter()
                .filter(|protocol| !protocol.is_empty())
                .cloned()
                .collect();
            if protocols.is_empty() {
                continue;
            }
            for name in [&model.raw_model, &model.standard_model]
                .into_iter()
                .flatten()
            {
                let target = entry.entry(name.clone()).or_default();
                target.extend(protocols.iter().cloned());
            }
        }
    }
    ModelProtocolCapabilities { groups }
}

/// 按需取某个站点的模型协议能力图（well-known → snapshot → 建图）。
///
/// 与广场摘要的周期刷新**互不相干**：这里服务的是「验证弹窗打开那一刻」，
/// 用户站可能不在受管名单里（自建站照样有公开协议），不值得为它进缓存——
/// 弹窗开一次抓一次。任何一步失败返回 `None`（站点没有公开数据 ⇒ 预筛
/// 全部按 Unknown，行为退回无预筛）。
pub async fn model_protocol_capabilities(site_origin: &str) -> Option<ModelProtocolCapabilities> {
    let host = crate::relay::aff::lookup_host(site_origin);
    if host.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("LoongPort/relay-transit")
        .build()
        .ok()?;
    let well_known: WellKnown = fetch_json(&client, &well_known_url(&host)).await.ok()?;
    let snapshot_url = validate_well_known(&well_known).ok()?;
    let snapshot: TransitSnapshot = fetch_json(&client, snapshot_url.as_str()).await.ok()?;
    Some(build_model_capabilities(&snapshot))
}

/// 周期刷新：并发抓全部受管站的 transit 摘要并合并进缓存。
///
/// 失败的站**保留旧值**（见模块文档的缓存语义），全程不向上抛错——
/// 这是后台数据面，失败的表现只是「该站这轮没有新数字」。
pub async fn refresh_for_hosts(hosts: &[String]) {
    use futures::StreamExt;

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("LoongPort/relay-transit")
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            log::warn!("ai-transit 客户端建不起来，本轮跳过: {error}");
            return;
        }
    };

    let results = futures::stream::iter(hosts.iter().cloned().map(|host| {
        let client = client.clone();
        async move {
            let result = fetch_summary(&client, &well_known_url(&host)).await;
            (host, result)
        }
    }))
    .buffer_unordered(REFRESH_CONCURRENCY)
    .collect::<Vec<(String, Result<TransitSummary, AppError>)>>()
    .await;

    let mut cache = read_cache();
    let (updated, failed) = apply_results(&mut cache, results);
    // 摘要全空也落盘：把「这轮全军覆没」与「从没跑过」区分开没有价值，
    // 但写一次空缓存可以省掉每轮对不存在文件的重复解析。
    if let Err(error) = write_cache(&cache) {
        log::warn!("ai-transit 缓存写不进去（下轮重试）: {error}");
    }
    log::info!(
        "{}",
        crate::diagnostics::DiagnosticEvent::new("relay.transit_refresh", "done")
            .field("hosts", hosts.len())
            .field("updated", updated)
            .field("failed", failed)
    );
}

/// 把一轮抓取结果合并进缓存：成功的站覆盖，失败的站**保留旧值**。
///
/// 纯函数（不打网络不碰磁盘），「失败不擦旧值」这条语义单测钉住。
fn apply_results(
    cache: &mut TransitCache,
    results: Vec<(String, Result<TransitSummary, AppError>)>,
) -> (usize, usize) {
    let mut updated = 0usize;
    let mut failed = 0usize;
    for (host, result) in results {
        match result {
            Ok(summary) => {
                cache.entries.insert(host, summary);
                updated += 1;
            }
            Err(error) => {
                failed += 1;
                log::debug!("ai-transit 刷新失败（保留旧值） {host}: {error}");
            }
        }
    }
    (updated, failed)
}

/// `pub(crate)`：`leaderboard::tests` 的 decorate 用例要直接操纵这份缓存
/// （写一条摘要、用 home 隔离挡住真实用户目录）。
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use serial_test::serial;

    /// 缓存文件落在 `get_home_dir` 下——与 `leaderboard` 测试同一条纪律：
    /// 用 `CC_SWITCH_TEST_HOME` 把它指到临时目录，跑完还原。
    /// `pub(crate)`：guard 必须在调用方作用域里活到断言结束（提前 Drop
    /// 会把 home 还原，后续读取就打到真实用户目录）。
    pub(crate) struct TestHomeGuard(Option<std::ffi::OsString>);

    impl TestHomeGuard {
        fn set(tag: &str) -> Self {
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            let dir = std::env::temp_dir().join(format!("lp-transit-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::env::set_var("CC_SWITCH_TEST_HOME", &dir);
            Self(previous)
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    /// 把 `get_home_dir` 隔离到临时目录（跨模块测试助手）。
    pub(crate) fn transit_cache_guard(tag: &str) -> TestHomeGuard {
        TestHomeGuard::set(tag)
    }

    /// 用真实投影解析一份快照 JSON 并建能力图（跨模块测试助手：
    /// `model_verification::target` 的预筛测试借它守住字段名契约，
    /// 不手搓 Map）。
    pub(crate) fn capabilities_from_snapshot_json(raw: &str) -> ModelProtocolCapabilities {
        let snapshot: TransitSnapshot =
            serde_json::from_str(raw).expect("测试快照 JSON 要能过投影");
        build_model_capabilities(&snapshot)
    }

    /// 往当前缓存合并一条摘要（跨模块测试助手）。
    pub(crate) fn write_transit_cache_entry(host: &str, summary: TransitSummary) {
        let mut cache = read_cache();
        cache.entries.insert(host.to_string(), summary);
        write_cache(&cache).expect("写测试缓存");
    }

    /// 真实快照的字段子集（中性域名），含新老两种 sub2api 倍率形态。
    fn snapshot_json(
        composite: Option<f64>,
        rate: Option<f64>,
        availability: Option<f64>,
    ) -> String {
        let group = format!(
            r#"{{"name":"g1"{}{}}}"#,
            composite
                .map(|v| format!(r#","composite_multiplier":{v}"#))
                .unwrap_or_default(),
            rate.map(|v| format!(r#","rate_multiplier":{v}"#))
                .unwrap_or_default(),
        );
        let monitor = format!(
            r#"{{"name":"g1"{}}}"#,
            availability
                .map(|v| format!(r#","availability_7d":{v}"#))
                .unwrap_or_default(),
        );
        format!(
            r#"{{"schema_version":"ai-transit.v1","generated_at":"2026-08-21T00:00:00Z","groups":[{group}],"monitoring":[{monitor}]}}"#
        )
    }

    #[test]
    fn summarize_prefers_composite_and_falls_back_to_rate() {
        let snapshot: TransitSnapshot =
            serde_json::from_str(&snapshot_json(Some(0.5), Some(0.9), Some(95.0))).unwrap();
        assert_eq!(summarize(&snapshot).min_multiplier, Some(0.5));

        // 老版 sub2api：只有 rate_multiplier。
        let legacy: TransitSnapshot =
            serde_json::from_str(&snapshot_json(None, Some(0.3), Some(97.5))).unwrap();
        assert_eq!(summarize(&legacy).min_multiplier, Some(0.3));

        // 都没有 → None，而不是 0（0 会渲染成「免费」）。
        let bare: TransitSnapshot = serde_json::from_str(&snapshot_json(None, None, None)).unwrap();
        assert_eq!(summarize(&bare).min_multiplier, None);
        assert_eq!(summarize(&bare).min_availability_7d, None);
    }

    #[test]
    fn summarize_takes_the_worst_group_and_filters_dirty_values() {
        let raw = r#"{
            "generated_at": "2026-08-21T00:00:00Z",
            "groups": [
                {"composite_multiplier": 0.06},
                {"composite_multiplier": 0.18},
                {"composite_multiplier": 0.0},
                {"composite_multiplier": -1.0}
            ],
            "monitoring": [
                {"availability_7d": 95.0},
                {"availability_7d": 88.3},
                {"availability_7d": 250.0}
            ]
        }"#;
        let snapshot: TransitSnapshot = serde_json::from_str(raw).unwrap();
        let summary = summarize(&snapshot);
        assert_eq!(summary.min_multiplier, Some(0.06));
        assert_eq!(summary.min_availability_7d, Some(88.3));
        assert_eq!(
            summary.synced_at,
            chrono::DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
                .unwrap()
                .timestamp()
        );
    }

    #[test]
    fn well_known_requires_exact_schema_and_https_snapshot() {
        let ok = WellKnown {
            schema_version: SCHEMA_VERSION.into(),
            snapshot_url: "https://api.example.com/api/public/transit/v1/snapshot".into(),
        };
        // 跨子域（相对 well-known 所在主机）放行——实测存在这种合法部署。
        assert!(validate_well_known(&ok).is_ok());

        let wrong_version = WellKnown {
            schema_version: "ai-transit.v2".into(),
            snapshot_url: ok.snapshot_url.clone(),
        };
        assert!(validate_well_known(&wrong_version).is_err());

        let http = WellKnown {
            schema_version: SCHEMA_VERSION.into(),
            snapshot_url: "http://api.example.com/snapshot".into(),
        };
        assert!(validate_well_known(&http).is_err());
    }

    #[test]
    fn snapshot_parsing_tolerates_missing_arrays_and_unknown_fields() {
        let snapshot: TransitSnapshot =
            serde_json::from_str(r#"{"station":{"name":"Example"},"future_field":1}"#).unwrap();
        let summary = summarize(&snapshot);
        assert_eq!(summary.min_multiplier, None);
        assert_eq!(summary.min_availability_7d, None);
        assert_eq!(
            summary.synced_at, 0,
            "没有 generated_at 就是 0，不是解析失败"
        );
    }

    #[tokio::test]
    #[serial]
    async fn fetch_summary_walks_well_known_then_snapshot_end_to_end() {
        // 本地 axum 服务验证传输与整链拼装。本地服务是 http，而 snapshot_url
        // 的 HTTPS 闸是硬规则，所以这里的 well-known 指向一个 https 地址
        // （过闸后即生产会真打的形态），快照抓取用同一个 fetch_json 打本地
        // 端点——两者共用的「传输 + 体积闸 + 解析」路径完全一致。
        let app = axum::Router::new()
            .route(
                "/.well-known/ai-transit.json",
                axum::routing::get(|| async move {
                    axum::Json(serde_json::json!({
                        "schema_version": "ai-transit.v1",
                        "system": "sub2api",
                        "snapshot_url": "https://snapshot.example/api/public/transit/v1/snapshot",
                    }))
                }),
            )
            .route(
                "/snapshot",
                axum::routing::get(|| async move {
                    let body = snapshot_json(Some(0.06), None, Some(95.0));
                    axum::Json(serde_json::from_str::<serde_json::Value>(&body).unwrap())
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:19999")
            .await
            .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = reqwest::Client::new();
        let well_known: WellKnown = fetch_json(
            &client,
            "http://127.0.0.1:19999/.well-known/ai-transit.json",
        )
        .await
        .expect("well-known 要能抓回并解析");
        let snapshot_url = validate_well_known(&well_known).expect("https snapshot_url 要过闸");
        assert_eq!(
            snapshot_url.as_str(),
            "https://snapshot.example/api/public/transit/v1/snapshot"
        );
        let snapshot: TransitSnapshot = fetch_json(&client, "http://127.0.0.1:19999/snapshot")
            .await
            .expect("快照要能抓回并解析");
        let summary = summarize(&snapshot);
        assert_eq!(summary.min_multiplier, Some(0.06));
        assert_eq!(summary.min_availability_7d, Some(95.0));
        assert!(summary.synced_at > 0);

        // well-known 404 的站必须整链失败（Err），而不是部分成功。
        let missing =
            fetch_json::<WellKnown>(&client, "http://127.0.0.1:19999/.well-known/missing.json")
                .await;
        assert!(missing.is_err());

        server.abort();
    }

    #[test]
    #[serial]
    fn cache_roundtrip_keeps_stale_entries_for_failed_hosts() {
        let _guard = TestHomeGuard::set("stale");

        let mut cache = TransitCache::default();
        cache.entries.insert(
            "a.example".into(),
            TransitSummary {
                min_multiplier: Some(0.1),
                min_availability_7d: Some(90.0),
                synced_at: 100,
            },
        );
        write_cache(&cache).unwrap();

        let loaded = read_cache();
        assert_eq!(
            loaded.entries.get("a.example").unwrap().min_multiplier,
            Some(0.1)
        );
        assert!(summaries().contains_key("a.example"));

        // schema 版本不匹配的旧缓存整体作废（宁可没徽章也不能拿错口径）。
        let wrong = TransitCache {
            schema_version: 99,
            entries: cache.entries.clone(),
        };
        let bytes = serde_json::to_vec(&wrong).unwrap();
        crate::config::atomic_write(&cache_path(), &bytes).unwrap();
        assert!(read_cache().entries.is_empty());
    }

    /// 「失败的站保留旧值」是徽章不闪烁的根基，纯函数直接钉住。
    #[test]
    fn apply_results_overwrites_successes_and_keeps_failures_stale() {
        let mut cache = TransitCache::default();
        cache.entries.insert(
            "flaky.example".into(),
            TransitSummary {
                min_multiplier: Some(0.9),
                min_availability_7d: Some(70.0),
                synced_at: 1,
            },
        );
        cache.entries.insert(
            "stable.example".into(),
            TransitSummary {
                min_multiplier: Some(0.5),
                min_availability_7d: Some(80.0),
                synced_at: 1,
            },
        );

        let fresh = TransitSummary {
            min_multiplier: Some(0.06),
            min_availability_7d: Some(95.0),
            synced_at: 2,
        };
        let (updated, failed) = apply_results(
            &mut cache,
            vec![
                ("stable.example".into(), Ok(fresh)),
                (
                    "flaky.example".into(),
                    Err(AppError::Config("本轮抓取失败".into())),
                ),
                ("new.example".into(), Ok(fresh)),
            ],
        );

        assert_eq!((updated, failed), (2, 1));
        assert_eq!(cache.entries.get("stable.example").unwrap().synced_at, 2);
        assert_eq!(
            cache.entries.get("flaky.example").unwrap().min_multiplier,
            Some(0.9),
            "失败的站必须保留上一轮旧值，不能被擦掉"
        );
        assert!(cache.entries.contains_key("new.example"));
    }

    /// 打**线上真实站点**验生产链路（well-known_url 的 https 前缀 + reqwest
    /// 重定向 + 真实快照体量）。**默认不跑**（CI 不依赖外网）。
    ///
    /// 站点清单从环境变量注入（`TRANSIT_LIVE_HOSTS=host1,host2`），测试代码
    /// 里不写真实域名——与 `live_remote_config` 同一条纪律。
    ///
    /// 手动跑：`TRANSIT_LIVE_HOSTS=… cargo test --lib live_transit -- --ignored --nocapture`
    #[test]
    #[ignore = "需要外网；手动跑 --ignored（站点经 TRANSIT_LIVE_HOSTS 注入）"]
    fn live_transit_refresh_fetches_real_summaries() {
        let hosts: Vec<String> = std::env::var("TRANSIT_LIVE_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_string)
            .collect();
        assert!(!hosts.is_empty(), "没有可测站点：设 TRANSIT_LIVE_HOSTS");

        let _guard = TestHomeGuard::set("live");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建 runtime");
        rt.block_on(refresh_for_hosts(&hosts));

        let summaries = summaries();
        for host in &hosts {
            match summaries.get(host) {
                Some(summary) => println!(
                    "  {host}: 倍率 {:?} / 可用性 {:?}",
                    summary.min_multiplier, summary.min_availability_7d
                ),
                None => println!("  {host}: 无摘要（该站可能未部署公开协议）"),
            }
        }
        assert!(
            summaries
                .values()
                .any(|summary| summary.min_multiplier.is_some()),
            "一个倍率都没抓到——生产链路（https 前缀/重定向/解析）可能有回归"
        );
    }
}
