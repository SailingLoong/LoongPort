//! 中转站广场：受管名单 × 站方公开数据 × 自家实测的**纯读取投影**。
//!
//! ## 三个数据源，各有各的 owner
//!
//! | 数据 | owner | 刷新触发 |
//! |---|---|---|
//! | 行的存在性（受管名单） | 远端配置（`remote_config`，Ed25519 验签） | maintenance 周期 + 启动 |
//! | 站方一手摘要（价格 / 可用性） | 各站 ai-transit 公开协议（[`crate::relay::transit`]） | maintenance 周期 + 手动刷新 |
//! | 实测观测（首字 / 错误率） | 自家众测快照（[`crate::crowd::snapshot`]） | maintenance 周期 + 读路径 SWR |
//!
//! 广场自己**没有任何抓取物**：不 fetch、不落缓存、无 TTL——每次读取都是
//! 「三份现成缓存 + 探针记录」的投影。2026-09-07 换源定形：veridrop HTML
//! 抓取层整体下线（四张分榜、详情页兜底、4 份 6h 缓存全删），观测层改读
//! 自家众测数据；`LeaderboardKind`（协议分榜）随之消失，广场只有一张列表。
//!
//! ## 门禁分层（2026-09-07 拍板）
//!
//! 实测数据在官网实测页完全公开，读侧门禁只可能是激励设计而非隐私：
//! **行级观测**（TTFT / 错误率徽章、名次）人人可见——本模块的 crowd 装饰
//! 不查共建开关；**深数据**（近 7 天 / 时段 / 分布，详情弹窗）保留共建门禁
//! （`crowd_get_snapshot` 关共建返 `None`，见 [`crate::commands::crowd`]）。
//!
//! ## 排序
//!
//! 名次 = 实测健康序：有 w24 实测者按 TTFT p50 升序（错误率次键，字段缺席
//! 视为最差），无实测者垫底按 host 字典序钉死——全部行重排 `1..N`
//! （见 [`renumber_ranks`]）。不在客户端发明 0-100 健康评分：排序只需要
//! 序，不需要分。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::error::AppError;
use crate::relay::remote_config::{RelayDirectoryPolicy, RemoteConfig};

/// 广场行的实测观测投影（crowd 快照的 w24 窗口）。
///
/// 两个字段都可缺席（窗口内没有 TTFT 样本 / 没有错误样本）——缺席不渲染
/// 对应徽章，**不能拿 0 当「无数据」**（0% 错误率是真实可能的观测值）。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrowdSummary {
    pub ttft_p50_ms: Option<f64>,
    pub err_rate: Option<f64>,
}

/// 广场的一行。行存在性由**受管名单**决定（[`apply_policy`]），两份观测数据
/// （`crowd` / `transit`）都可缺席——缺席只是徽章不渲染，行仍在：手动添加
/// 入口已删的当下，广场行是用户接入受管站的唯一入口（2026-09-06 维护者
/// 拍板「白名单是展示的充分条件」，换源自建数据后观测缺席只会更常见）。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayDirectoryItem {
    pub site_host: String,
    /// 站点**身份**：注册域（apex，[`crate::relay::identity::site_domain`] 的产出）。
    /// 与 `site_host`（真实 host）分工：身份给跨数据源的 join（实测快照的站点键），
    /// 真实 host 给链接与取数——别拿一个当另一个用。
    pub site_domain: String,
    pub display_name: String,
    /// 本广场内的位置（1..N，读取时按实测健康序重排）。
    pub rank: u32,
    /// 自家实测观测（近 24 小时，crowd 快照按 apex join）。`None` = 该站没有
    /// 过 k-匿的 w24 窗口（或快照里根本没有它）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crowd: Option<CrowdSummary>,
    pub entry_url: String,
    /// 站方一手 transit 摘要（ai-transit.v1）。`None`/缺席 = 该站没有公开协议
    /// 数据（New API 系站点），前端不渲染徽章。由 [`decorate_transit`] 在
    /// **读取路径**合并，与 crowd 快照各有各的刷新周期。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transit: Option<crate::relay::transit::TransitSummary>,
}

/// 返回给 UI 的广场列表。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayDirectoryListing {
    pub items: Vec<RelayDirectoryItem>,
    /// 实测快照的数据时间（`generatedAt`，站方无关的自家口径）；没有快照时
    /// 为 0，前端不显示时间戳。
    pub synced_at: i64,
}

fn normalized_policy(policy: &RelayDirectoryPolicy) -> RelayDirectoryPolicy {
    RelayDirectoryPolicy {
        blocked_hosts: policy
            .blocked_hosts
            .iter()
            .map(|host| crate::relay::identity::request_host(host))
            .filter(|host| !host.is_empty())
            .collect(),
        sites: policy
            .sites
            .iter()
            .map(|(host, site)| (crate::relay::identity::request_host(host), site.clone()))
            .filter(|(host, _)| !host.is_empty())
            .collect(),
    }
}

/// 受管站点的**真实 origin** host 全集（sponsors ∪ aff_codes ∪ promo_codes ∪
/// relay_directory，统一归一后去 blocked）。
///
/// ⭐ 三份清单的**唯一源**（wawapi.top 实测踩出的洞：aff 名单里的站进了广场，
/// 导入闸却只认 relay_directory ⇒ 广场显示、点接入被拒）。广场行源
/// （[`apply_policy`]）、目录导入闸的回落匹配（`commands::relay::directory_entry_source`）、
/// 探针名单（[`refresh_site_probes_for_directory`]）全部从这份派生——任何一份
/// 单独收窄，就会重现「广场显示但点不进 / 探针名单漏探」，三处必须同宽。
pub(crate) fn managed_site_hosts(config: &RemoteConfig) -> Vec<String> {
    let policy = normalized_policy(&config.relay_directory);
    let blocked: BTreeSet<_> = policy.blocked_hosts.iter().cloned().collect();
    let mut hosts: BTreeSet<String> = policy.sites.into_keys().collect();
    for sponsor in &config.sponsors {
        hosts.insert(crate::relay::identity::request_host(&sponsor.site_origin));
    }
    hosts.extend(
        config
            .aff_codes
            .keys()
            .map(|host| crate::relay::identity::request_host(host)),
    );
    hosts.extend(
        config
            .promo_codes
            .keys()
            .map(|host| crate::relay::identity::request_host(host)),
    );
    hosts
        .into_iter()
        .filter(|host| !host.is_empty() && !blocked.contains(host))
        .collect()
}

/// 探针落盘 + 逐站日志：排查「这个站为什么不在广场」时翻日志就能回答
/// （三分类：网络失败不摘、协议认不出连续多轮才摘，见
/// [`crate::relay::site_probe`]）。由 `maintenance` 的 directory 周期任务调用。
pub(crate) async fn refresh_site_probes_for_directory() {
    let config = crate::relay::remote_config::load_cached().unwrap_or_default();
    let managed_hosts = managed_site_hosts(&config);
    if managed_hosts.is_empty() {
        log::info!(
            "{}",
            crate::diagnostics::DiagnosticEvent::new("relay.directory_funnel", "skipped")
                .field("reason", "no_managed_sites")
        );
        return;
    }

    let origins = managed_hosts
        .iter()
        .map(|host| format!("https://{host}"))
        .collect::<Vec<_>>();
    for record in crate::relay::site_probe::probe_and_record(&origins).await {
        log::info!(
            "{}",
            crate::diagnostics::DiagnosticEvent::new("relay.directory_funnel", "probe")
                .field("host", record.host)
                .field("verdict", format!("{:?}", record.verdict))
                .field("backend", format!("{:?}", record.backend))
                .field("consecutive_panel_misses", record.consecutive_panel_misses)
                .field("exposed", record.exposed)
                .field("detail", record.detail)
        );
    }
}

/// 广场行源：**每个受管站一行**（按注册域去重），观测数据全部由读取路径
/// 的 decorate 步骤合并。
///
/// 名字与入口优先取 `relay_directory` 条目（`display_name` / `entry_url`），
/// 缺席回落 host 本身（想更体面随时补目录条目）。去重按注册域：sponsors 与
/// relay_directory 常各录一形（www. vs 裸域），同一 apex 是一站一行；受管名单
/// 的集合序里裸域先到，天然选中目录条目那一形。
fn apply_policy(config: &RemoteConfig) -> Vec<RelayDirectoryItem> {
    let policy = normalized_policy(&config.relay_directory);
    let mut seen_domains = BTreeSet::new();
    managed_site_hosts(config)
        .into_iter()
        .filter_map(|site_host| {
            let site_domain = crate::relay::identity::site_domain(&site_host);
            if !seen_domains.insert(site_domain.clone()) {
                return None;
            }
            let site = policy.sites.get(&site_host);
            let display_name = site
                .and_then(|site| site.display_name.as_deref())
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| site_host.clone());
            let entry_url = site
                .and_then(|site| site.entry_url.as_deref())
                .filter(|url| url::Url::parse(url).is_ok_and(|url| url.scheme() == "https"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://{site_host}"));
            Some(RelayDirectoryItem {
                site_host,
                site_domain,
                display_name,
                rank: 0,
                crowd: None,
                entry_url,
                transit: None,
            })
        })
        .collect()
}

/// 曝光闸：白名单（`apply_policy`）之后再过一遍**探针健康**——连续多轮「协议认不出」
/// 的站从广场摘掉。见 [`crate::relay::site_probe`] 的三分类（网络失败不摘）。
fn apply_probe_gate(items: Vec<RelayDirectoryItem>) -> Vec<RelayDirectoryItem> {
    let store = crate::relay::site_probe::SiteProbeStore::load();
    filter_probe_gated(items, &store)
}

/// `apply_probe_gate` 的纯函数核——测试直接喂 store，不碰磁盘。
fn filter_probe_gated(
    items: Vec<RelayDirectoryItem>,
    store: &crate::relay::site_probe::SiteProbeStore,
) -> Vec<RelayDirectoryItem> {
    items
        .into_iter()
        .filter(|item| store.should_expose(&item.site_host))
        .collect()
}

/// 把站方一手 transit 摘要合并进广场行（join 键 = 归一后的 site host）。
///
/// 只在**读取路径**调用，transit 有自己的刷新周期，读取只合并已有摘要。
/// 数据最多「旧一个周期」，比「因为刚抓取失败就整个消失」好——徽章闪没闪现
/// 比数字旧几小时更伤信任。
fn decorate_transit(mut items: Vec<RelayDirectoryItem>) -> Vec<RelayDirectoryItem> {
    let summaries = crate::relay::transit::summaries();
    if summaries.is_empty() {
        return items;
    }
    for item in &mut items {
        item.transit = summaries.get(&item.site_host).cloned();
    }
    items
}

/// 把自家实测观测合并进广场行（join 键 = 注册域 `site_domain`，与上报侧
/// 的站点身份同源）。行级观测**公开**（不查共建开关，见模块文档「门禁分层」）。
///
/// 纯函数核：测试直接喂快照，不碰磁盘。
fn decorate_crowd_with(
    mut items: Vec<RelayDirectoryItem>,
    snapshot: Option<&crate::crowd::snapshot::Snapshot>,
) -> Vec<RelayDirectoryItem> {
    let Some(snapshot) = snapshot else {
        return items;
    };
    for item in &mut items {
        let crowd = snapshot
            .sites
            .get(&item.site_domain)
            .and_then(|stats| stats.w24.as_ref())
            .map(|w24| CrowdSummary {
                ttft_p50_ms: w24.ttft_p50_ms,
                err_rate: w24.err_rate,
            })
            // 两个观测字段都缺席的 w24（过 k-匿但零样本）与「没有数据」同待遇：
            // 排序里把它当无数据行垫底，而不是凭空排到有数据的站前面。
            .filter(|crowd| crowd.ttft_p50_ms.is_some() || crowd.err_rate.is_some());
        item.crowd = crowd;
    }
    items
}

/// 广场最终呈现序：按实测健康序重排名次（1..N）。
///
/// 排序键：TTFT p50 升序 → 错误率升序（字段缺席视为 +∞，即最差），两键都
/// 平的按 host 字典序钉死顺序（快照测试友好，两次渲染不跳行）。无实测的行
/// 两个键都是 +∞，自然垫在一切有实测者之后。
fn renumber_ranks(mut items: Vec<RelayDirectoryItem>) -> Vec<RelayDirectoryItem> {
    fn ttft(item: &RelayDirectoryItem) -> f64 {
        item.crowd
            .as_ref()
            .and_then(|crowd| crowd.ttft_p50_ms)
            .unwrap_or(f64::INFINITY)
    }
    fn err_rate(item: &RelayDirectoryItem) -> f64 {
        item.crowd
            .as_ref()
            .and_then(|crowd| crowd.err_rate)
            .unwrap_or(f64::INFINITY)
    }
    items.sort_by(|a, b| {
        ttft(a)
            .partial_cmp(&ttft(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                err_rate(a)
                    .partial_cmp(&err_rate(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.site_host.cmp(&b.site_host))
    });
    for (index, item) in items.iter_mut().enumerate() {
        item.rank = index as u32 + 1;
    }
    items
}

/// 广场读取的唯一入口：三份缓存 + 探针记录 → 排序后的列表投影。
///
/// 全程本地（无网络往返），命令层直接同步调用；快照陈旧/缺失时由命令层
/// 后台追新（SWR），读取永远先出画面。
pub fn read_listing() -> Result<RelayDirectoryListing, AppError> {
    let config = crate::relay::remote_config::load_cached().unwrap_or_default();
    let snapshot = crate::crowd::snapshot::read_cached();
    let synced_at = snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.generated_at);
    let items = renumber_ranks(decorate_transit(decorate_crowd_with(
        apply_probe_gate(apply_policy(&config)),
        snapshot.as_ref(),
    )));
    Ok(RelayDirectoryListing { items, synced_at })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::BTreeMap;

    fn config_with_directory() -> RemoteConfig {
        RemoteConfig {
            relay_directory: RelayDirectoryPolicy {
                blocked_hosts: vec!["blocked.example".into()],
                sites: BTreeMap::from([(
                    "790053500.com".into(),
                    crate::relay::remote_config::RelayDirectorySite {
                        entry_url: Some("https://790053500.com/keys".into()),
                        purchase_url: None,
                        usage_url: None,
                        display_name: Some("鑫旺".into()),
                    },
                )]),
            },
            ..RemoteConfig::default()
        }
    }

    fn snapshot_with(
        domain: &str,
        w24: Option<crate::crowd::snapshot::WindowStats>,
    ) -> crate::crowd::snapshot::Snapshot {
        let mut sites = std::collections::BTreeMap::new();
        sites.insert(
            domain.to_string(),
            crate::crowd::snapshot::SiteStats {
                w24,
                w7: None,
                hours: vec![],
            },
        );
        crate::crowd::snapshot::Snapshot {
            version: 1,
            generated_at: 1_786_680_000,
            sites,
            ttft_bin_edges: vec![],
        }
    }

    fn window(ttft: Option<f64>, err: Option<f64>) -> crate::crowd::snapshot::WindowStats {
        crate::crowd::snapshot::WindowStats {
            samples: 42,
            sources: 1,
            ttft_p50_ms: ttft,
            ttft_p95_ms: None,
            err_rate: err,
            cache_hit_rate: None,
            cost_usd_per_m_tok: None,
            ttft_bins: vec![],
        }
    }

    /// 每个受管站一行：观测缺席只是徽章不亮，行必须在——「白名单 = 展示充分
    /// 条件」的换源版（此前钉的是「零 veridrop 数据」，veridrop 下线后同一
    /// 断言换到零 crowd 数据上，语义不变）。
    #[test]
    fn managed_sites_get_a_row_with_zero_crowd_data() {
        let items = apply_policy(&config_with_directory());

        assert_eq!(items.len(), 1, "blocked 站除外，受管站都要有行");
        let item = &items[0];
        assert_eq!(item.site_host, "790053500.com");
        assert_eq!(item.display_name, "鑫旺");
        assert_eq!(item.entry_url, "https://790053500.com/keys");
        assert_eq!(item.crowd, None);
        assert_eq!(item.transit, None);
    }

    #[test]
    fn sponsor_sites_fall_back_to_their_origin() {
        let config = RemoteConfig {
            sponsors: vec![crate::relay::remote_config::Sponsor {
                site_origin: "https://wawazz.xyz".into(),
                display_name: "WAWA ZZ API".into(),
                tagline: String::new(),
            }],
            ..RemoteConfig::default()
        };

        let items = apply_policy(&config);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].site_host, "wawazz.xyz");
        // 行名沿用旧语义：目录条目优先，否则 host 本身（sponsor 的展示名只喂
        // 首启推荐屏，不进广场行——想体面随时补目录条目）。
        assert_eq!(items[0].display_name, "wawazz.xyz");
        assert_eq!(items[0].entry_url, "https://wawazz.xyz");
    }

    #[test]
    fn aff_only_sites_get_a_row_named_by_host() {
        let config = RemoteConfig {
            aff_codes: BTreeMap::from([("aijws.example".to_string(), "CODE".to_string())]),
            ..RemoteConfig::default()
        };

        let items = apply_policy(&config);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].site_host, "aijws.example");
        assert_eq!(items[0].display_name, "aijws.example");
        assert_eq!(items[0].entry_url, "https://aijws.example");
    }

    /// 同一注册域一站一行：sponsors 与 relay_directory 常各录一形（www. vs
    /// 裸域）。受管名单的集合序里裸域先到，目录条目的名字/入口优先。
    #[test]
    fn one_site_domain_one_row_even_when_whitelisted_twice() {
        let mut config = config_with_directory();
        config.sponsors = vec![crate::relay::remote_config::Sponsor {
            site_origin: "https://www.790053500.com".into(),
            display_name: "sponsor 形态".into(),
            tagline: String::new(),
        }];

        let items = apply_policy(&config);

        assert_eq!(items.len(), 1, "同一注册域不能出两行");
        assert_eq!(items[0].display_name, "鑫旺");
    }

    /// 用仓内真实远端配置（`remote-config/public/v1/config.json`，线上部署那份的
    /// 源）做端到端实证：实测快照完全缺席时，**每一个受管站点都有行**。配置侧
    /// 键形漂移、身份归一化回退、行源逻辑被改坏，都会在这里红。
    #[test]
    fn every_site_in_the_shipped_config_gets_a_row_with_zero_crowd_data() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../remote-config/public/v1/config.json");
        let config: RemoteConfig = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|error| panic!("读不到 {:?}: {error}", path)),
        )
        .expect("仓内 v1 配置必须能被客户端 schema 解析");

        let items = apply_policy(&config);

        let mut covered_domains: BTreeSet<String> =
            items.iter().map(|item| item.site_domain.clone()).collect();
        // blocked_hosts 是维护者的显式 kill switch，不算「漏斗拦」。
        let blocked: BTreeSet<String> = config
            .relay_directory
            .blocked_hosts
            .iter()
            .map(|host| crate::relay::identity::site_domain(host))
            .collect();
        let managed_domains: BTreeSet<String> = managed_site_hosts(&config)
            .into_iter()
            .map(|host| crate::relay::identity::site_domain(&host))
            .filter(|domain| !blocked.contains(domain))
            .collect();
        for domain in &managed_domains {
            assert!(
                covered_domains.remove(domain),
                "受管域 {domain} 在零实测数据下没有广场行"
            );
        }
        assert!(
            covered_domains.is_empty(),
            "多出了非受管的行: {covered_domains:?}"
        );
    }

    /// 三份清单的唯一源：sponsors / aff / promo / directory **全部**要进这份名单，
    /// 统一归一（www、大小写）并去 blocked。**会红的改法**：把任何一类从并集里
    /// 拿掉 —— 那正是 wawapi.top 式的洞（aff 名单进了广场，探针/导入闸却漏了它）。
    #[test]
    fn managed_site_hosts_spans_sponsors_aff_promo_and_directory() {
        let config = RemoteConfig {
            relay_directory: RelayDirectoryPolicy {
                blocked_hosts: vec!["blocked.example".into()],
                sites: BTreeMap::from([(
                    "bestapi.store".into(),
                    crate::relay::remote_config::RelayDirectorySite::default(),
                )]),
            },
            sponsors: vec![crate::relay::remote_config::Sponsor {
                site_origin: "https://www.WawAPII.com".into(),
                display_name: "WawAPI".into(),
                tagline: String::new(),
            }],
            aff_codes: BTreeMap::from([("wawapi.top".to_string(), "AFF".into())]),
            promo_codes: BTreeMap::from([("promo.example".to_string(), "PROMO".into())]),
            ..RemoteConfig::default()
        };

        let hosts = managed_site_hosts(&config);
        for expected in [
            "bestapi.store",
            "wawapii.com",
            "wawapi.top",
            "promo.example",
        ] {
            assert!(
                hosts.contains(&expected.to_string()),
                "受管全集缺 {expected}：{hosts:?}"
            );
        }
        assert!(!hosts.contains(&"blocked.example".to_string()));
    }

    /// 曝光闸：连续多轮「协议认不出」的站从广场摘掉；网络失败 / 没探过的站不摘。
    /// 钉的是 `filter_probe_gated` 与 `SiteProbeStore::should_expose` 的合谋行为。
    #[test]
    fn probe_gate_hides_sites_with_consecutive_panel_misses() {
        use crate::relay::site_probe::{
            ProbeOutcome, ProbeVerdict, SiteProbeStore, HIDE_AFTER_CONSECUTIVE_PANEL_MISSES,
        };

        fn outcome(verdict: ProbeVerdict) -> ProbeOutcome {
            ProbeOutcome {
                verdict,
                backend: None,
                detail: "test".into(),
                probed_at: 1,
            }
        }

        let mut store = SiteProbeStore::default();
        for _ in 0..HIDE_AFTER_CONSECUTIVE_PANEL_MISSES {
            store.record("koozhan.example", &outcome(ProbeVerdict::UnrecognizedPanel));
        }
        store.record("cf-blocked.example", &outcome(ProbeVerdict::NetworkBlocked));

        let items = vec!["koozhan.example", "cf-blocked.example", "fresh.example"]
            .into_iter()
            .map(|site_host| RelayDirectoryItem {
                site_host: site_host.into(),
                site_domain: crate::relay::identity::site_domain(site_host),
                display_name: site_host.into(),
                rank: 1,
                crowd: None,
                entry_url: format!("https://{site_host}"),
                transit: None,
            })
            .collect::<Vec<_>>();

        let remaining = filter_probe_gated(items, &store)
            .into_iter()
            .map(|item| item.site_host)
            .collect::<Vec<_>>();

        assert!(!remaining.contains(&"koozhan.example".to_owned()));
        assert!(remaining.contains(&"cf-blocked.example".to_owned()));
        assert!(remaining.contains(&"fresh.example".to_owned()));
    }

    /// 实测装饰按注册域 join：有 w24 的站带观测、w24 没过 k-匿（`None`）与
    /// 根本不在快照里的站同待遇（不渲染）、两个观测字段都空的窗口也不算有数据
    /// （排序里当无数据行，不凭空压到有数据的站前面）。
    #[test]
    fn decorate_crowd_joins_by_site_domain() {
        let snapshot = {
            let mut snapshot =
                snapshot_with("present.example", Some(window(Some(812.5), Some(0.008))));
            snapshot.sites.insert(
                "anon.example".into(),
                crate::crowd::snapshot::SiteStats {
                    w24: None,
                    w7: None,
                    hours: vec![],
                },
            );
            snapshot.sites.insert(
                "empty.example".into(),
                crate::crowd::snapshot::SiteStats {
                    w24: Some(window(None, None)),
                    w7: None,
                    hours: vec![],
                },
            );
            snapshot
        };
        fn row(site_host: &str) -> RelayDirectoryItem {
            RelayDirectoryItem {
                site_host: site_host.into(),
                site_domain: crate::relay::identity::site_domain(site_host),
                display_name: site_host.into(),
                rank: 1,
                crowd: None,
                entry_url: format!("https://{site_host}"),
                transit: None,
            }
        }

        let items = decorate_crowd_with(
            vec![
                row("present.example"),
                row("anon.example"),
                row("empty.example"),
                row("absent.example"),
            ],
            Some(&snapshot),
        );

        assert_eq!(
            items[0].crowd,
            Some(CrowdSummary {
                ttft_p50_ms: Some(812.5),
                err_rate: Some(0.008),
            })
        );
        assert_eq!(items[1].crowd, None, "w24 没过 k-匿 = 无观测");
        assert_eq!(items[2].crowd, None, "两个观测字段都空 = 无观测");
        assert_eq!(items[3].crowd, None, "不在快照里 = 无观测");
    }

    /// 名次 = 实测健康序：TTFT 升序（缺席最差）→ 错误率次键 → host 字典序
    /// 钉死；无实测的行垫底。全部行 1..N 连续。
    #[test]
    fn renumber_ranks_sorts_by_measured_health() {
        fn row(site_host: &str, ttft: Option<f64>, err: Option<f64>) -> RelayDirectoryItem {
            RelayDirectoryItem {
                site_host: site_host.into(),
                site_domain: crate::relay::identity::site_domain(site_host),
                display_name: site_host.into(),
                rank: 0,
                crowd: (ttft.is_some() || err.is_some()).then_some(CrowdSummary {
                    ttft_p50_ms: ttft,
                    err_rate: err,
                }),
                entry_url: format!("https://{site_host}"),
                transit: None,
            }
        }

        let items = vec![
            row("no-data-b.example", None, None),
            row("slow.example", Some(2100.0), Some(0.001)),
            row("tie-b.example", Some(812.0), Some(0.009)),
            row("no-ttft.example", None, Some(0.002)),
            row("tie-a.example", Some(812.0), Some(0.009)),
            row("fast.example", Some(511.0), Some(0.02)),
            row("no-data-a.example", None, None),
        ];

        let renumbered = renumber_ranks(items);
        let order = renumbered
            .iter()
            .map(|item| (item.site_host.as_str(), item.rank))
            .collect::<Vec<_>>();

        assert_eq!(
            order,
            vec![
                ("fast.example", 1),  // 511ms 最快，错误率高也先按 TTFT 排
                ("tie-a.example", 2), // 812ms 并列，host 字典序钉死
                ("tie-b.example", 3),
                ("slow.example", 4),      // 2100ms
                ("no-ttft.example", 5),   // TTFT 缺席 = 最差，但错误率在场仍算有数据
                ("no-data-a.example", 6), // 无实测垫底，host 字典序
                ("no-data-b.example", 7),
            ],
            "实测健康序 + 连续名次"
        );
    }

    /// transit 摘要按归一 host 精确 join：有摘要的站带上、没有的保持 `None`
    /// （徽章不渲染），摘要缓存为空时整体短路（新装首启的常态）。
    #[test]
    #[serial]
    fn decorate_transit_joins_by_host_and_short_circuits_when_empty() {
        fn row(site_host: &str) -> RelayDirectoryItem {
            RelayDirectoryItem {
                site_host: site_host.into(),
                site_domain: crate::relay::identity::site_domain(site_host),
                display_name: site_host.into(),
                rank: 1,
                crowd: None,
                entry_url: format!("https://{site_host}"),
                transit: None,
            }
        }

        // 空缓存短路：一条都不该被碰（短路分支连 entries 都不查）。
        let untouched = decorate_transit(vec![row("a.example")]);
        assert!(untouched[0].transit.is_none());

        // 写一份只含 a.example 的摘要缓存，join 后只有它带徽章数据。
        // guard 必须活到断言之后：提前 Drop 会还原 home、让 decorate 读到
        // 真实用户目录里的缓存。
        let _guard = crate::relay::transit::tests::transit_cache_guard("decorate");
        crate::relay::transit::tests::write_transit_cache_entry(
            "a.example",
            crate::relay::transit::tests::badge_summary(Some(0.06), Some(95.0)),
        );
        let decorated = decorate_transit(vec![row("a.example"), row("b.example")]);
        let summary = decorated[0].transit.clone().expect("a.example 应带上摘要");
        assert_eq!(summary.min_multiplier, Some(0.06));
        assert!(decorated[1].transit.is_none(), "没有摘要的站保持 None");
    }
}
