//! 中转站广场的可见性：**一个开关**（`settings.plaza_visible`），两个播种点。
//!
//! ## 归因模型（2026-09-06 拍板）
//!
//! 广场是「冷启动发现面」：从站长教程引流来的用户（首启「手填域名」弹窗填的
//! 就是那家站的域名）已经完成了发现，默认不该看到别家；没有归属的用户（直接
//! 下载、填的是我们不认识的域）才需要广场。落地成**开关的默认值**，不是独立
//! 的展示层：
//!
//! - 弹窗提交的域名（apex 归一）命中受保护域名 → 开关默认**关**；否则默认**开**；
//! - 用户此后手动翻转（窄命令 [`crate::commands::settings`]），翻转结果永远优先；
//! - 存量安装升级后没有弹窗可命中，补一次播种（2026-09-07 定调）：**全部**
//!   已配置站点都是受保护站 → 默认关；掺了任何一个非保护站（哪怕只有一家）
//!   → 默认开；一个站都没有 → 保持未播种（= 展示）。存量无从知道首站归因，
//!   「清一色受保护站」才是纯伙伴漏斗的可信信号，配过别家就是逛站用户。
//!
//! ## 受保护域名：显式名单优先（`protected_hosts`），缺省回落并集

//!
//! 受保护是维护者的**关系态**（哪些站长的引流要保护），不是「受管」的自动
//! 推论 —— v2 配置的 `protected_hosts` 非空就是那份名单；缺省回落四源并集
//! （**不减 blocked**：blocked 是展示策略，站长关系还在，被屏蔽站的用户照样
//! 来自那家站）。两份清单语义不同，故意不共享。
//!
//! ## 触发归属
//!
//! 播种是数据层行为：存量补播种挂在启动（maintenance 一次性任务）；弹窗播种
//! 挂在用户提交动作本身（那是一次显式写入，不是视图读路径的副作用）。
//! 广场页只读 settings 里的 `plaza_visible`（`None` = 展示），不驱动任何刷新。
//!
//! ## 受保护名单拿不到时两个播种点都**不播**
//!
//! 名单未知（离线 / 端点故障且无缓存）时任何域名都判「未命中」，播下去会把
//! 伙伴用户永久误播成开（写-if-None，之后无人纠正）。留着 `None` 交给后续
//! 启动补播 —— 可见行为不变（`None` = 展示），但名单到位后还能播对。

use std::collections::BTreeSet;

use crate::relay::identity::site_domain;
use crate::relay::remote_config::{self, RemoteConfig};

/// 归因判定的「受保护域名」全集，按注册域（apex）归一。
///
/// 配置里的 `protected_hosts` **非空 ⇒ 就是这份**：受保护是维护者的**关系态**
/// （哪些站长正在引流、要保护），不是「受管」的自动推论 —— 改名单 = 改远端
/// v2 配置，无需发版。缺省/为空 ⇒ 回落四源并集（sponsors ∪ aff_codes ∪
/// promo_codes ∪ relay_directory.sites，**不减 blocked**，理由见模块文档）。
/// 回落方向有意保守：拿不到名单时宁可多保护。由此「不保护任何人」是
/// 不可表达状态 —— 那是危险向（每个站长的漏斗都裸奔），有意够不着。
pub(crate) fn protected_site_domains(config: &RemoteConfig) -> BTreeSet<String> {
    let explicit: BTreeSet<String> = config
        .protected_hosts
        .iter()
        .map(|host| site_domain(host))
        .filter(|domain| !domain.is_empty())
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    let mut domains: BTreeSet<String> = config
        .sponsors
        .iter()
        .map(|sponsor| site_domain(&sponsor.site_origin))
        .collect();
    domains.extend(config.aff_codes.keys().map(|host| site_domain(host)));
    domains.extend(config.promo_codes.keys().map(|host| site_domain(host)));
    domains.extend(
        config
            .relay_directory
            .sites
            .keys()
            .map(|host| site_domain(host)),
    );
    domains.into_iter().filter(|d| !d.is_empty()).collect()
}

/// 纯判定：首启「手填域名」弹窗归因出的开关默认值。
///
/// `false` = 命中受保护域名（来自某家站，默认关）；`true` = 未命中（默认开）。
/// 子域归 apex 后比对（`api.example.com` 与 `example.com` 是同一家站）。
fn first_site_default(domain: &str, config: &RemoteConfig) -> bool {
    !protected_site_domains(config).contains(&site_domain(domain))
}

/// 纯判定：存量安装补播种的结果。`None` = 不播种。
///
/// **全部**站点都是受保护站（子域也算）→ `Some(false)`；掺了任何一个非保护
/// 站（哪怕只有一家）→ `Some(true)`；一个站都没有 → `None`（未归因，保持
/// 「展示」默认）。存量无从知道首站归因，「清一色受保护」是纯伙伴漏斗的唯一
/// 可信信号；配过别家说明是逛站/比价用户 —— 广场正是给他们的。
fn existing_install_default(origins: &[String], config: &RemoteConfig) -> Option<bool> {
    if origins.is_empty() {
        return None;
    }
    let protected = protected_site_domains(config);
    let all_protected = origins
        .iter()
        .all(|origin| protected.contains(&site_domain(origin)));
    Some(!all_protected)
}

/// 首启「手填域名」弹窗提交的播种点（写-if-None，归因一次性）。
///
/// 弹窗只在首启出现，那时远端配置可能还没拉过（maintenance 有启动延迟），
/// 而归因判据就是这份配置 —— 花一次有上界的拉取（8s 超时）换正确归因。
/// 拉不到就回落缓存；**缓存也没有（离线新装）则本进程不播**（见模块文档
/// 「名单拿不到时不播」）—— 留着 `None` 交给后续启动的存量补播种纠正。
pub async fn seed_from_first_site(domain: &str) {
    let config = remote_config::refresh_and_cache()
        .await
        .or_else(remote_config::load_cached);
    if let Some(config) = config {
        seed_if_unsent(first_site_default(domain, &config));
    }
}

/// 存量安装升级后的补播种点（写-if-None）。调用方传入用户已配置的站点 origin
/// 全集；触发 = 启动（见模块文档「触发归属」）。
///
/// 先刷一次配置再判：升级后的**首次启动**连世代缓存都还没有（缓存按世代
/// 命名），空配置下「所有站都算非保护」会把纯伙伴用户误播成开。刷新失败且
/// 无缓存则本启动不播（同上，「名单拿不到时不播」）。
pub async fn seed_for_existing_install(relay_origins: &[String]) {
    if crate::settings::get_settings().plaza_visible.is_some() {
        return;
    }
    let config = remote_config::refresh_and_cache()
        .await
        .or_else(remote_config::load_cached);
    if let Some(config) = config {
        if let Some(visible) = existing_install_default(relay_origins, &config) {
            seed_if_unsent(visible);
        }
    }
}

/// 写-if-None 的播种核：开关一旦有值（播种过或用户翻过），播种永远不再碰它。
///
/// 双重检查（锁外的快路径 + `mutate_settings` 锁内复读）防两个播种点并发；
/// 锁内复读依赖 [`crate::settings::mutate_settings`] 的 RMW 语义。
fn seed_if_unsent(visible: bool) {
    if crate::settings::get_settings().plaza_visible.is_some() {
        return;
    }
    if let Err(e) = crate::settings::mutate_settings(|settings| {
        if settings.plaza_visible.is_none() {
            settings.plaza_visible = Some(visible);
        }
    }) {
        log::warn!("广场开关播种失败（保持默认展示）: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_sources() -> RemoteConfig {
        let mut config = RemoteConfig::default();
        config
            .aff_codes
            .insert("panel.example.com".into(), "CODE".into());
        config
            .promo_codes
            .insert("shop.example.org".into(), "PROMO".into());
        config
            .relay_directory
            .sites
            .insert("relay.example.net".into(), Default::default());
        config
    }

    #[test]
    fn explicit_protected_hosts_override_the_derived_union() {
        let mut config = config_with_sources();

        // 非空名单 = 就是这份：并集里的其他站不再受保护（维护者只承诺了两家）。
        config.protected_hosts = vec!["airelay.buzz".into(), "api.790053500.com".into()];
        assert_eq!(
            protected_site_domains(&config),
            ["790053500.com", "airelay.buzz"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert!(!first_site_default("https://airelay.buzz", &config));
        // 并集里的站（example.com）在显式名单之外 ⇒ 未命中 ⇒ 默认开。
        assert!(first_site_default("https://panel.example.com", &config));

        // 空名单（含只写了空串的退化形态）= 回落四源并集。
        config.protected_hosts = vec![];
        assert!(protected_site_domains(&config).contains("example.com"));
        config.protected_hosts = vec![String::new()];
        assert!(protected_site_domains(&config).contains("example.com"));
    }

    #[test]
    fn protected_domains_are_the_apex_of_all_four_sources() {
        let protected = protected_site_domains(&config_with_sources());
        // 子域录入按注册域收拢：四源各一形、三个 apex。
        assert_eq!(
            protected,
            ["example.com", "example.org", "example.net"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn first_site_hits_subdomain_of_a_protected_site() {
        let config = config_with_sources();
        // 用户拿到的是面板子域 —— 与裸域同一家站。
        assert!(!first_site_default(
            "https://panel.example.com/login",
            &config
        ));
        // 留空弹窗回退的官方站也是受保护域名之一（统一规则，无特判）。
        assert!(!first_site_default(
            "bestapi.store",
            &config_helper_official()
        ));
        // 完全不认识的域 → 默认开。
        assert!(first_site_default(
            "https://api.unknown.example.dev",
            &config
        ));
    }

    fn config_helper_official() -> RemoteConfig {
        let mut config = RemoteConfig::default();
        config
            .aff_codes
            .insert("bestapi.store".into(), "OFFICIAL".into());
        config
    }

    #[test]
    fn blocked_status_does_not_affect_attribution() {
        // 归因全集不减 blocked：墓碑期间（全量 blocked）站长关系仍在，
        // 来自那些站的用户照样默认关。
        let mut config = config_with_sources();
        config.relay_directory.blocked_hosts = vec![
            "panel.example.com".into(),
            "shop.example.org".into(),
            "relay.example.net".into(),
        ];
        assert!(!first_site_default("https://panel.example.com", &config));
    }

    #[test]
    fn existing_install_seeds_hidden_only_when_every_configured_site_is_protected() {
        let config = config_with_sources();

        // 一个站都没有 → 不播种（未归因，= 展示）。
        assert_eq!(existing_install_default(&[], &config), None);

        // 清一色受保护站（子域也算、多家也行）→ 默认关：纯伙伴漏斗的可信信号。
        assert_eq!(
            existing_install_default(&["https://api.example.com".into()], &config),
            Some(false)
        );
        assert_eq!(
            existing_install_default(
                &[
                    "https://panel.example.com".into(),
                    "https://shop.example.org".into(),
                    "https://relay.example.net".into()
                ],
                &config
            ),
            Some(false)
        );

        // 掺了任何一个非保护站（哪怕只有一家）→ 默认开：逛站/比价用户。
        assert_eq!(
            existing_install_default(&["https://self.example.io".into()], &config),
            Some(true)
        );
        assert_eq!(
            existing_install_default(
                &[
                    "https://api.example.com".into(),
                    "https://self.example.io".into()
                ],
                &config
            ),
            Some(true)
        );
    }

    #[test]
    fn empty_config_treats_everything_as_unattributed() {
        // 纯判定在空配置下：人人「不认识」→ 默认开。**调用方**在名单拿不到时
        // 根本不进这个分支（见模块文档「名单拿不到时不播」）—— 这里钉的是
        // 纯函数自身的合同，防止有人绕过那道守卫直接拿空配置播种。
        assert!(first_site_default(
            "anything.example.com",
            &RemoteConfig::default()
        ));
        assert_eq!(
            existing_install_default(
                &["https://any.example.com".into()],
                &RemoteConfig::default()
            ),
            Some(true)
        );
    }
}
