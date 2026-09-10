//! Plaza visibility is owned by settings. Unclassified installations stay hidden.
//! The first explicitly submitted valid domain is retained before network access.
//! Startup and remote-config maintenance retry that domain; manual choices win.

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
/// 站（哪怕只有一家）→ `Some(true)`；一个站都没有 → `None`（未归因，保持关闭）。存量无从知道首站归因，「清一色受保护」是纯伙伴漏斗的唯一
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

fn remember_first_site(settings: &mut crate::settings::AppSettings, domain: &str) {
    if settings.plaza_visible.is_none() && settings.plaza_first_site_domain.is_none() {
        settings.plaza_first_site_domain = Some(domain.to_string());
    }
}

fn apply_pending(settings: &mut crate::settings::AppSettings, config: &RemoteConfig) {
    if settings.plaza_visible.is_none() {
        if let Some(domain) = settings.plaza_first_site_domain.take() {
            settings.plaza_visible = Some(first_site_default(&domain, config));
        }
    }
}

/// Explicit submission records attribution before attempting the remote lookup.
pub async fn seed_from_first_site(domain: &str) -> Result<(), crate::error::AppError> {
    let origin = crate::relay::sub2api::normalize_site_origin(domain)?;
    let domain = site_domain(&origin);
    crate::settings::mutate_settings(|settings| remember_first_site(settings, &domain))?;
    if crate::settings::get_settings().plaza_visible.is_some() {
        return Ok(());
    }
    let config = remote_config::refresh_and_cache()
        .await
        .or_else(remote_config::load_cached);
    if let Some(config) = config {
        resolve_pending(&config)?;
    }
    Ok(())
}

/// Called by data-layer startup and config maintenance, never a view read.
pub(crate) fn resolve_pending(config: &RemoteConfig) -> Result<(), crate::error::AppError> {
    let settings = crate::settings::get_settings();
    if settings.plaza_visible.is_none() && settings.plaza_first_site_domain.is_some() {
        crate::settings::mutate_settings(|settings| apply_pending(settings, config))?;
    }
    Ok(())
}

fn apply_startup_attribution(
    settings: &mut crate::settings::AppSettings,
    config: &RemoteConfig,
    relay_origins: &[String],
) {
    if settings.plaza_visible.is_some() {
        return;
    }
    if settings.plaza_first_site_domain.is_some() {
        apply_pending(settings, config);
    } else if settings.service_onboarding_completed {
        settings.plaza_visible = existing_install_default(relay_origins, config);
    }
}

pub async fn seed_for_existing_install(relay_origins: &[String]) {
    if crate::settings::get_settings().plaza_visible.is_some() {
        return;
    }
    let config = remote_config::refresh_and_cache()
        .await
        .or_else(remote_config::load_cached);
    if let Some(config) = config {
        // Re-read the first domain under the write lock after the network await.
        if let Err(error) = crate::settings::mutate_settings(|settings| {
            apply_startup_attribution(settings, &config, relay_origins);
        }) {
            log::warn!("Could not persist plaza attribution: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_uses_current_pending_domain_before_historical_accounts() {
        let config = config_with_sources();
        let origins = vec!["https://other.example".into()];
        let mut settings = crate::settings::AppSettings {
            service_onboarding_completed: true,
            ..Default::default()
        };
        // A first submission arriving while config is in flight beats history.
        remember_first_site(&mut settings, "example.com");
        let restored = serde_json::to_value(&settings).unwrap();
        settings = serde_json::from_value(restored).unwrap();
        apply_startup_attribution(&mut settings, &config, &origins);
        assert_eq!(settings.plaza_visible, Some(false));
        assert_eq!(settings.plaza_first_site_domain, None);
        // An explicit toggle arriving during that same fetch beats both sources.
        settings.plaza_visible = Some(true);
        settings.plaza_first_site_domain = Some("example.com".into());
        apply_startup_attribution(&mut settings, &config, &origins);
        assert_eq!(settings.plaza_visible, Some(true));
    }

    #[test]
    fn first_domain_is_retained_until_classified_and_manual_choice_wins() {
        let mut settings = crate::settings::AppSettings::default();
        remember_first_site(&mut settings, "panel.example.com");
        remember_first_site(&mut settings, "other.example.org");
        assert_eq!(
            settings.plaza_first_site_domain.as_deref(),
            Some("panel.example.com")
        );
        assert_eq!(settings.plaza_visible, None);
        apply_pending(&mut settings, &config_with_sources());
        assert_eq!(settings.plaza_visible, Some(false));
        assert_eq!(settings.plaza_first_site_domain, None);
        settings.plaza_visible = Some(true);
        remember_first_site(&mut settings, "panel.example.com");
        apply_pending(&mut settings, &config_with_sources());
        assert_eq!(settings.plaza_visible, Some(true));
    }

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
        config.protected_hosts = vec!["protected.example".into(), "api.partner.example".into()];
        assert_eq!(
            protected_site_domains(&config),
            ["partner.example", "protected.example"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert!(!first_site_default("https://protected.example", &config));
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
