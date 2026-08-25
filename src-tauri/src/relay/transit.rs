//! sub2api 站点的 ai-transit.v1 一手数据：`/.well-known/ai-transit.json` 发现 +
//! snapshot 快照解析，产出广场行的价格/可用性徽章与站点详情弹窗要展示的
//! 摘要（充值口径、逐分组倍率/缓存命中/可用性/延迟、来源披露）。
//!
//! ## 投影边界：缓存到分组粒度，不缓存逐模型价格表
//!
//! 快照的体积大头是逐模型 USD/token 四价表（一份几十 KB）。分组粒度已覆盖
//! 「这家怎么充值、哪个分组便宜、稳不稳」的全部展示需求；跨站同模型比价
//! 才需要逐模型数据，那是另一个产品形态，真做再单独设计。60 点监测时间线
//! 只覆盖 5 小时、price_trend 实测与配置倍率相同，都不带价值，不缓存。
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

/// v2：摘要从「两个数字」扩成含充值口径 / 逐分组摘要 / 来源披露的完整
/// 投影（详情弹窗消费）。旧缓存整体作废，下一轮刷新重建。
const CACHE_SCHEMA_VERSION: u8 = 2;

/// 唯一认的协议版本。站点未来出 v2 时这里不会误读——版本不匹配按
/// 「该站没有 transit 数据」处理，而不是拿错口径的数字展示给用户。
const SCHEMA_VERSION: &str = "ai-transit.v1";

/// 广场行徽章 + 详情弹窗共用的站点摘要。行徽章取**最保守值**（最低倍率 /
/// 最低分组可用性）：用户按徽章做的是「这家最便宜多少、最差稳到什么程度」
/// 的预期，用均值会把最差分组藏起来；弹窗消费其余字段（充值口径、逐分组、
/// 披露），全部来自同一份快照投影。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitSummary {
    /// 各分组综合倍率的最小值（`composite_multiplier`，老版 sub2api 只有
    /// `rate_multiplier`，兜底取它）。`None` = 快照里没有可用的倍率字段。
    pub min_multiplier: Option<f64>,
    /// 各分组可用性的最小值（0-100）。窗口按站方发布口径依次偏好
    /// 7d → 15d → 30d → 1d：新版 sub2api 只发布 1d/15d/30d，只认 7d 会让
    /// 这些站的可用性徽章整个消失。
    pub min_availability: Option<f64>,
    /// 快照的 `generated_at`（站方口径的数据时间，不是我们抓取的时间）。
    pub synced_at: i64,
    /// 充值系数：1 单位本币兑多少 USD 额度（`billing.recharge_multiplier`）。
    pub recharge_multiplier: Option<f64>,
    /// 最低充值额（本币，币种见 `currency`）。
    pub minimum_top_up: Option<f64>,
    /// 本币币种（`billing.currency`，如 CNY）。
    pub currency: Option<String>,
    /// 来源披露：上游类型（站方自报原值，如 official / mixed / reverse）。
    pub upstream_type: Option<String>,
    /// 来源披露：是否逆向账号池。
    pub is_reverse: Option<bool>,
    /// 站方公开价格页（快照 `station.price_url` 原值，通常是 /public/transit）。
    pub price_url: Option<String>,
    /// 站方客服入口（`station.support_url` 原值，通常是 TG 群）。
    pub support_url: Option<String>,
    /// 逐分组摘要（详情弹窗的表格行）。无名分组是脏数据，跳过。
    pub groups: Vec<TransitGroupSummary>,
}

/// 单个分组的展示摘要。倍率 / 缓存命中来自 `groups[]` 区块，可用性 / 延迟
/// 从 `monitoring[]` 按分组名 join（新版快照监测条目带 `group_name`，老版
/// 只有与分组同名的 `name`，两个键都认）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitGroupSummary {
    pub name: String,
    /// 平台（anthropic / openai / grok…，站方原值；空 = 快照没给）。
    pub platform: String,
    /// 综合倍率（老版 sub2api 兜底 `rate_multiplier`）。
    pub multiplier: Option<f64>,
    /// 近 7 日缓存命中率（0-100，站方按真实流量统计）。
    pub cache_hit_rate_7d: Option<f64>,
    /// 可用性（0-100，窗口偏好链同 [`TransitSummary::min_availability`]）。
    pub availability: Option<f64>,
    /// 平均延迟毫秒（优先 7 日窗口，兜底 1 日）。
    pub avg_latency_ms: Option<f64>,
    /// 该分组发布的模型条目数。
    pub model_count: usize,
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
    station: TransitStation,
    #[serde(default)]
    billing: TransitBilling,
    #[serde(default)]
    groups: Vec<TransitGroup>,
    #[serde(default)]
    monitoring: Vec<TransitMonitor>,
    #[serde(default)]
    disclosure: TransitDisclosure,
    #[serde(default)]
    generated_at: String,
}

#[derive(Debug, Default, Deserialize)]
struct TransitStation {
    #[serde(default)]
    price_url: Option<String>,
    #[serde(default)]
    support_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitBilling {
    #[serde(default)]
    recharge_multiplier: Option<f64>,
    #[serde(default)]
    minimum_top_up: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitDisclosure {
    #[serde(default)]
    upstream_type: Option<String>,
    #[serde(default)]
    is_reverse: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    composite_multiplier: Option<f64>,
    #[serde(default)]
    rate_multiplier: Option<f64>,
    #[serde(default)]
    cache_usage: Option<TransitCacheUsage>,
    /// 逐模型条目（能力查询消费；摘要只需要它的长度）。
    #[serde(default)]
    models: Vec<TransitGroupModel>,
}

/// 分组的缓存命中统计，只消费 7 日窗口（24h/total 展示价值重复）。
#[derive(Debug, Default, Deserialize)]
struct TransitCacheUsage {
    #[serde(default)]
    last_7d: Option<TransitCachePeriod>,
}

#[derive(Debug, Default, Deserialize)]
struct TransitCachePeriod {
    #[serde(default)]
    cache_hit_rate: Option<f64>,
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

/// 监测条目（一个分组一条）。可用性窗口老版给 7d/15d/30d，新版
/// （api_gateway 系统）只给 1d/15d/30d——字段全部可缺，取数走窗口偏好链。
#[derive(Debug, Default, Deserialize)]
struct TransitMonitor {
    /// 老版快照里与分组同名的键；新版另带 `group_name`，join 时后者优先。
    #[serde(default)]
    name: String,
    #[serde(default)]
    group_name: Option<String>,
    #[serde(default)]
    availability_7d: Option<f64>,
    #[serde(default)]
    availability_15d: Option<f64>,
    #[serde(default)]
    availability_30d: Option<f64>,
    #[serde(default)]
    availability_1d: Option<f64>,
    #[serde(default)]
    avg_latency_7d_ms: Option<f64>,
    #[serde(default)]
    avg_latency_1d_ms: Option<f64>,
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

/// 可用性窗口偏好链：7d → 15d → 30d → 1d，脏值（越界 / 非有限）跳过。
fn best_availability(monitor: &TransitMonitor) -> Option<f64> {
    [
        monitor.availability_7d,
        monitor.availability_15d,
        monitor.availability_30d,
        monitor.availability_1d,
    ]
    .into_iter()
    .flatten()
    .find(|value| (0.0..=100.0).contains(value) && value.is_finite())
}

/// 平均延迟：优先 7 日窗口，兜底 1 日。
fn best_avg_latency(monitor: &TransitMonitor) -> Option<f64> {
    [monitor.avg_latency_7d_ms, monitor.avg_latency_1d_ms]
        .into_iter()
        .flatten()
        .find(|value| *value >= 0.0 && value.is_finite())
}

/// 监测条目按分组名建索引（`group_name` 优先，老版用与分组同名的 `name`）。
fn monitors_by_group(monitoring: &[TransitMonitor]) -> BTreeMap<&str, &TransitMonitor> {
    monitoring
        .iter()
        .filter_map(|monitor| {
            let key = monitor
                .group_name
                .as_deref()
                .filter(|name| !name.is_empty())
                .or_else(|| (!monitor.name.is_empty()).then_some(monitor.name.as_str()))?;
            Some((key, monitor))
        })
        .collect()
}

/// 分组倍率：composite 优先，老版兜底 rate；0 与负数是脏数据，过滤。
fn group_multiplier(group: &TransitGroup) -> Option<f64> {
    group
        .composite_multiplier
        .or(group.rate_multiplier)
        .filter(|multiplier| *multiplier > 0.0 && multiplier.is_finite())
}

fn fold_min(values: impl Iterator<Item = f64>) -> Option<f64> {
    values.fold(None::<f64>, |acc, value| {
        Some(match acc {
            Some(current) => current.min(value),
            None => value,
        })
    })
}

/// 从快照算摘要。纯函数，测试直接喂 [`TransitSnapshot`] 的 JSON。
fn summarize(snapshot: &TransitSnapshot) -> TransitSummary {
    let monitors = monitors_by_group(&snapshot.monitoring);
    let groups: Vec<TransitGroupSummary> = snapshot
        .groups
        .iter()
        .filter(|group| !group.name.is_empty())
        .map(|group| {
            let monitor = monitors.get(group.name.as_str()).copied();
            TransitGroupSummary {
                name: group.name.clone(),
                platform: group.platform.clone(),
                multiplier: group_multiplier(group),
                cache_hit_rate_7d: group
                    .cache_usage
                    .as_ref()
                    .and_then(|usage| usage.last_7d.as_ref())
                    .and_then(|period| period.cache_hit_rate)
                    .filter(|rate| (0.0..=100.0).contains(rate) && rate.is_finite()),
                availability: monitor.and_then(best_availability),
                avg_latency_ms: monitor.and_then(best_avg_latency),
                model_count: group.models.len(),
            }
        })
        .collect();
    // 行徽章对**全部**分组取最小（无名分组进不了表格，但它的低价也是真实
    // 发布价格，藏起来会美化「最便宜多少」的承诺）；表格投影才过滤无名分组。
    // 可用性同理：对全部监测条目取最小，不只看进了分组表的。
    let min_multiplier = fold_min(snapshot.groups.iter().filter_map(group_multiplier));
    let min_availability = fold_min(snapshot.monitoring.iter().filter_map(best_availability));
    TransitSummary {
        min_multiplier,
        min_availability,
        synced_at: parse_timestamp(&snapshot.generated_at),
        recharge_multiplier: snapshot
            .billing
            .recharge_multiplier
            .filter(|value| *value > 0.0 && value.is_finite()),
        minimum_top_up: snapshot
            .billing
            .minimum_top_up
            .filter(|value| *value >= 0.0 && value.is_finite()),
        currency: snapshot
            .billing
            .currency
            .clone()
            .filter(|currency| !currency.is_empty()),
        upstream_type: snapshot
            .disclosure
            .upstream_type
            .clone()
            .filter(|upstream| !upstream.is_empty()),
        is_reverse: snapshot.disclosure.is_reverse,
        price_url: snapshot
            .station
            .price_url
            .clone()
            .filter(|url| !url.is_empty()),
        support_url: snapshot
            .station
            .support_url
            .clone()
            .filter(|url| !url.is_empty()),
        groups,
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

    /// 只关心两个徽章数字的测试用摘要（其余字段走空缺省）。
    /// `pub(crate)`：`leaderboard::tests` 的 decorate 用例也用它写缓存条目。
    pub(crate) fn badge_summary(
        min_multiplier: Option<f64>,
        min_availability: Option<f64>,
    ) -> TransitSummary {
        TransitSummary {
            min_multiplier,
            min_availability,
            synced_at: 0,
            recharge_multiplier: None,
            minimum_top_up: None,
            currency: None,
            upstream_type: None,
            is_reverse: None,
            price_url: None,
            support_url: None,
            groups: Vec::new(),
        }
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
        assert_eq!(summarize(&bare).min_availability, None);
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
        assert_eq!(summary.min_availability, Some(88.3));
        assert_eq!(
            summary.synced_at,
            chrono::DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
                .unwrap()
                .timestamp()
        );
    }

    /// 可用性窗口偏好链：7d 优先；只有 1d/15d/30d 的新版快照也必须出徽章
    /// （修复前新版站只发 1d/15d/30d，可用性徽章整个消失）。
    #[test]
    fn availability_prefers_7d_and_falls_back_to_shorter_windows() {
        let raw = r#"{
            "monitoring": [
                {"name": "g1", "availability_7d": 95.0, "availability_15d": 90.0},
                {"name": "g2", "availability_15d": 88.0, "availability_30d": 80.0},
                {"name": "g3", "availability_1d": 99.0}
            ]
        }"#;
        let snapshot: TransitSnapshot = serde_json::from_str(raw).unwrap();
        let summary = summarize(&snapshot);
        // 三个条目各自取到 95 / 88 / 99，行徽章取最保守的 88。
        assert_eq!(summary.min_availability, Some(88.0));
        assert_eq!(summary.groups.len(), 0, "没有分组区块就没有表格行");

        // 越界脏值不挡住后面窗口：7d 是 250（脏）→ 落到 15d。
        let dirty = r#"{
            "monitoring": [{"name": "g1", "availability_7d": 250.0, "availability_15d": 91.0}]
        }"#;
        let snapshot: TransitSnapshot = serde_json::from_str(dirty).unwrap();
        assert_eq!(summarize(&snapshot).min_availability, Some(91.0));
    }

    /// 详情弹窗消费的完整投影：充值口径 / 逐分组摘要（监测按分组名 join，
    /// group_name 与老版 name 两种键都认）/ 来源披露 / 站方链接。
    #[test]
    fn summarize_projects_groups_billing_and_disclosure() {
        let raw = r#"{
            "generated_at": "2026-08-25T00:00:00Z",
            "station": {
                "price_url": "https://panel.example/public/transit",
                "support_url": "https://t.me/example-group"
            },
            "billing": {
                "recharge_multiplier": 1.0,
                "minimum_top_up": 50.0,
                "currency": "CNY"
            },
            "disclosure": {"upstream_type": "mixed", "is_reverse": true},
            "groups": [
                {
                    "name": "group-a",
                    "platform": "openai",
                    "composite_multiplier": 0.1,
                    "cache_usage": {"last_7d": {"cache_hit_rate": 79.8}},
                    "models": [{"standard_model": "m1"}, {"standard_model": "m2"}]
                },
                {
                    "name": "group-b",
                    "platform": "anthropic",
                    "rate_multiplier": 0.3
                }
            ],
            "monitoring": [
                {
                    "name": "legacy-name",
                    "group_name": "group-a",
                    "availability_7d": 96.5,
                    "avg_latency_7d_ms": 4715
                },
                {
                    "name": "group-b",
                    "availability_1d": 100.0,
                    "avg_latency_1d_ms": 800
                }
            ]
        }"#;
        let snapshot: TransitSnapshot = serde_json::from_str(raw).unwrap();
        let summary = summarize(&snapshot);

        assert_eq!(summary.recharge_multiplier, Some(1.0));
        assert_eq!(summary.minimum_top_up, Some(50.0));
        assert_eq!(summary.currency.as_deref(), Some("CNY"));
        assert_eq!(summary.upstream_type.as_deref(), Some("mixed"));
        assert_eq!(summary.is_reverse, Some(true));
        assert_eq!(
            summary.price_url.as_deref(),
            Some("https://panel.example/public/transit")
        );
        assert_eq!(
            summary.support_url.as_deref(),
            Some("https://t.me/example-group")
        );

        assert_eq!(summary.groups.len(), 2);
        let group_a = &summary.groups[0];
        assert_eq!(group_a.name, "group-a");
        assert_eq!(group_a.platform, "openai");
        assert_eq!(group_a.multiplier, Some(0.1));
        assert_eq!(group_a.cache_hit_rate_7d, Some(79.8));
        // group_name 优先于 name（legacy-name 是老键，join 仍要命中 group-a）。
        assert_eq!(group_a.availability, Some(96.5));
        assert_eq!(group_a.avg_latency_ms, Some(4715.0));
        assert_eq!(group_a.model_count, 2);

        let group_b = &summary.groups[1];
        // 老版兜底：rate_multiplier；监测条目只有 name 键也照常 join。
        assert_eq!(group_b.multiplier, Some(0.3));
        assert_eq!(group_b.cache_hit_rate_7d, None);
        assert_eq!(group_b.availability, Some(100.0));
        assert_eq!(group_b.avg_latency_ms, Some(800.0));

        assert_eq!(summary.min_multiplier, Some(0.1));
        assert_eq!(summary.min_availability, Some(96.5));
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
        assert_eq!(summary.min_availability, None);
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
        assert_eq!(summary.min_availability, Some(95.0));
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
        let mut seeded = badge_summary(Some(0.1), Some(90.0));
        seeded.synced_at = 100;
        cache.entries.insert("a.example".into(), seeded);
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
        let mut flaky = badge_summary(Some(0.9), Some(70.0));
        flaky.synced_at = 1;
        let mut stable = badge_summary(Some(0.5), Some(80.0));
        stable.synced_at = 1;
        cache.entries.insert("flaky.example".into(), flaky);
        cache.entries.insert("stable.example".into(), stable);

        let mut fresh = badge_summary(Some(0.06), Some(95.0));
        fresh.synced_at = 2;
        let (updated, failed) = apply_results(
            &mut cache,
            vec![
                ("stable.example".into(), Ok(fresh.clone())),
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
                    "  {host}: 倍率 {:?} / 可用性 {:?} / 分组 {} / 充值系数 {:?}",
                    summary.min_multiplier,
                    summary.min_availability,
                    summary.groups.len(),
                    summary.recharge_multiplier
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
