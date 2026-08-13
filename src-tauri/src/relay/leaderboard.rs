use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::error::AppError;
use crate::relay::remote_config::{RelayDirectoryPolicy, RemoteConfig};

const VERIDROP_ORIGIN: &str = "https://veridrop.org";
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardKind {
    Overall,
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
    Gemini,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolScore {
    pub protocol: String,
    pub score: u8,
    pub samples: u32,
    pub verdict: Option<String>,
    pub report_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedLeaderboardItem {
    pub veridrop_host: String,
    pub rank: u32,
    pub score: u8,
    pub samples: u32,
    pub latest_date: String,
    pub detail_url: String,
    pub protocol_scores: Vec<ProtocolScore>,
    pub claude_signature_rate: Option<u8>,
    pub scenarios: Vec<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayDirectoryItem {
    pub site_host: String,
    pub veridrop_host: String,
    pub display_name: String,
    pub rank: u32,
    pub score: u8,
    pub samples: u32,
    pub latest_date: String,
    pub detail_url: String,
    pub protocol_scores: Vec<ProtocolScore>,
    pub claude_signature_rate: Option<u8>,
    pub scenarios: Vec<String>,
    pub issues: Vec<String>,
    pub entry_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayLeaderboard {
    pub kind: LeaderboardKind,
    pub items: Vec<RelayDirectoryItem>,
    pub synced_at: i64,
    pub from_cache: bool,
}

impl LeaderboardKind {
    fn path(self) -> &'static str {
        match self {
            Self::Overall => "/leaderboard",
            Self::Claude => "/leaderboard/claude",
            Self::OpenAi => "/leaderboard/openai",
            Self::Gemini => "/leaderboard/gemini",
        }
    }

    fn cache_name(self) -> &'static str {
        match self {
            Self::Overall => "overall",
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
        }
    }
}

fn selector(value: &str) -> Result<Selector, AppError> {
    Selector::parse(value)
        .map_err(|error| AppError::Config(format!("VeriDrop 选择器无效 {value}: {error}")))
}

fn text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_string()
}

fn parse_u32(value: &str) -> Option<u32> {
    value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn parse_score(value: &str) -> Option<u8> {
    parse_u32(value).and_then(|value| u8::try_from(value).ok())
}

fn absolute_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value)
        .or_else(|_| url::Url::parse(VERIDROP_ORIGIN).and_then(|base| base.join(value)))
        .ok()?;
    (url.scheme() == "https" && url.host_str() == Some("veridrop.org")).then(|| url.to_string())
}

pub fn parse_page(
    _kind: LeaderboardKind,
    html: &str,
    managed_hosts: &[String],
) -> Result<Vec<ParsedLeaderboardItem>, AppError> {
    let document = Html::parse_document(html);
    let row_selector = selector("article.lb-row[data-impression-domain]")?;
    let score_selector = selector(".lb-score-num")?;
    let meta_selector = selector(".lb-meta")?;
    let detail_selector = selector(".lb-domain a, .lb-detail-link")?;
    let protocol_selector = selector(".lb-proto-chip")?;
    let protocol_name_selector = selector(".lb-proto-name")?;
    let protocol_score_selector = selector(".lb-proto-score")?;
    let protocol_count_selector = selector(".lb-proto-count")?;
    let protocol_verdict_selector = selector(".lb-proto-verdict")?;
    let signature_selector = selector(".lb-proto-sig")?;
    let issue_selector = selector(".lb-issues code")?;
    let scenario_selector = selector(".lb-main > div > span[title]")?;
    let latest_date_pattern =
        regex::Regex::new(r"最近\s+(\d{4}-\d{2}-\d{2})").expect("static regex");

    let mut items = Vec::new();
    for row in document.select(&row_selector) {
        let host = row
            .value()
            .attr("data-impression-domain")
            .map(crate::relay::aff::lookup_host)
            .filter(|host| !host.is_empty())
            .ok_or_else(|| AppError::Config("VeriDrop 榜单行缺少域名".into()))?;
        let top = row.value().attr("data-impression-surface") == Some("leaderboard_top");
        if !top && !managed_hosts.iter().any(|managed| managed == &host) {
            continue;
        }
        let rank = row
            .value()
            .attr("data-impression-position")
            .and_then(parse_u32)
            .ok_or_else(|| AppError::Config(format!("VeriDrop 榜单行 {host} 缺少排名")))?;
        let score = row
            .select(&score_selector)
            .next()
            .and_then(|element| parse_score(&text(element)))
            .filter(|score| *score <= 100)
            .ok_or_else(|| AppError::Config(format!("VeriDrop 榜单行 {host} 分数无效")))?;
        let meta = row
            .select(&meta_selector)
            .next()
            .map(text)
            .unwrap_or_default();
        let samples = parse_u32(meta.split('次').next().unwrap_or_default()).unwrap_or(0);
        let latest_date = latest_date_pattern
            .captures(&meta)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();
        let detail_url = row
            .select(&detail_selector)
            .filter_map(|element| element.value().attr("href"))
            .find_map(absolute_url)
            .ok_or_else(|| AppError::Config(format!("VeriDrop 榜单行 {host} 缺少详情链接")))?;

        let mut protocol_scores = Vec::new();
        let mut claude_signature_rate = None;
        for protocol in row.select(&protocol_selector) {
            let Some(name) = protocol.select(&protocol_name_selector).next().map(text) else {
                continue;
            };
            let Some(protocol_score) = protocol
                .select(&protocol_score_selector)
                .next()
                .and_then(|element| parse_score(&text(element)))
                .filter(|score| *score <= 100)
            else {
                continue;
            };
            let protocol_samples = protocol
                .select(&protocol_count_selector)
                .next()
                .and_then(|element| parse_u32(&text(element)))
                .unwrap_or(0);
            let verdict = protocol.select(&protocol_verdict_selector).next().map(text);
            let report_url = protocol.value().attr("href").and_then(absolute_url);
            if name == "Claude" {
                claude_signature_rate = protocol
                    .select(&signature_selector)
                    .next()
                    .and_then(|element| parse_score(&text(element)));
            }
            protocol_scores.push(ProtocolScore {
                protocol: name,
                score: protocol_score,
                samples: protocol_samples,
                verdict,
                report_url,
            });
        }
        let scenarios = row
            .select(&scenario_selector)
            .map(text)
            .map(|value| {
                value
                    .trim_start_matches(|character: char| !character.is_alphanumeric())
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
            .collect();
        let issues = row.select(&issue_selector).map(text).collect();
        items.push(ParsedLeaderboardItem {
            veridrop_host: host,
            rank,
            score,
            samples,
            latest_date,
            detail_url,
            protocol_scores,
            claude_signature_rate,
            scenarios,
            issues,
        });
    }
    Ok(items)
}

fn normalized_policy(policy: &RelayDirectoryPolicy) -> RelayDirectoryPolicy {
    RelayDirectoryPolicy {
        blocked_hosts: policy
            .blocked_hosts
            .iter()
            .map(|host| crate::relay::aff::lookup_host(host))
            .filter(|host| !host.is_empty())
            .collect(),
        sites: policy
            .sites
            .iter()
            .map(|(host, site)| (crate::relay::aff::lookup_host(host), site.clone()))
            .filter(|(host, _)| !host.is_empty())
            .collect(),
    }
}

fn managed_veridrop_hosts(config: &RemoteConfig) -> Vec<String> {
    let policy = normalized_policy(&config.relay_directory);
    let mut hosts = BTreeSet::new();
    for sponsor in &config.sponsors {
        hosts.insert(crate::relay::aff::lookup_host(&sponsor.site_origin));
    }
    hosts.extend(
        config
            .aff_codes
            .keys()
            .map(|host| crate::relay::aff::lookup_host(host)),
    );
    hosts.extend(
        config
            .promo_codes
            .keys()
            .map(|host| crate::relay::aff::lookup_host(host)),
    );
    for (loongport_host, site) in &policy.sites {
        hosts.insert(
            site.veridrop_host
                .as_deref()
                .map(crate::relay::aff::lookup_host)
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| loongport_host.clone()),
        );
    }
    hosts.into_iter().filter(|host| !host.is_empty()).collect()
}

pub fn apply_policy(
    parsed: Vec<ParsedLeaderboardItem>,
    config: &RemoteConfig,
) -> Vec<RelayDirectoryItem> {
    let policy = normalized_policy(&config.relay_directory);
    let blocked: BTreeSet<_> = policy.blocked_hosts.iter().cloned().collect();
    let mut aliases = BTreeMap::new();
    for (loongport_host, site) in &policy.sites {
        let value = (loongport_host.clone(), site.clone());
        aliases.insert(loongport_host.clone(), value.clone());
        let veridrop_host = site
            .veridrop_host
            .as_deref()
            .map(crate::relay::aff::lookup_host)
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| loongport_host.clone());
        aliases.insert(veridrop_host, value);
    }
    let mut seen_sites = BTreeSet::new();

    parsed
        .into_iter()
        .filter_map(|item| {
            let (site_host, override_site) = aliases
                .get(&item.veridrop_host)
                .cloned()
                .map(|(host, site)| (host, Some(site)))
                .unwrap_or_else(|| (item.veridrop_host.clone(), None));
            if blocked.contains(&site_host) || blocked.contains(&item.veridrop_host) {
                return None;
            }
            if !seen_sites.insert(site_host.clone()) {
                return None;
            }
            let entry_url = override_site
                .as_ref()
                .and_then(|site| site.entry_url.clone())
                .filter(|url| url::Url::parse(url).is_ok_and(|url| url.scheme() == "https"))
                .unwrap_or_else(|| format!("https://{site_host}"));
            let display_name = override_site
                .and_then(|site| site.display_name)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| site_host.clone());
            Some(RelayDirectoryItem {
                site_host,
                veridrop_host: item.veridrop_host,
                display_name,
                rank: item.rank,
                score: item.score,
                samples: item.samples,
                latest_date: item.latest_date,
                detail_url: item.detail_url,
                protocol_scores: item.protocol_scores,
                claude_signature_rate: item.claude_signature_rate,
                scenarios: item.scenarios,
                issues: item.issues,
                entry_url,
            })
        })
        .collect()
}

fn cache_path(kind: LeaderboardKind) -> std::path::PathBuf {
    crate::config::get_home_dir()
        .join(crate::config::APP_DIR_NAME)
        .join(format!("veridrop-{}.json", kind.cache_name()))
}

fn read_cache(kind: LeaderboardKind) -> Option<RelayLeaderboard> {
    let path = cache_path(kind);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_PAGE_BYTES as u64 {
        return None;
    }
    let mut cached: RelayLeaderboard = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if cached.kind != kind {
        return None;
    }
    cached.from_cache = true;
    Some(cached)
}

fn apply_policy_to_cached(cached: RelayLeaderboard, config: &RemoteConfig) -> RelayLeaderboard {
    let parsed = cached
        .items
        .into_iter()
        .map(|item| ParsedLeaderboardItem {
            veridrop_host: item.veridrop_host,
            rank: item.rank,
            score: item.score,
            samples: item.samples,
            latest_date: item.latest_date,
            detail_url: item.detail_url,
            protocol_scores: item.protocol_scores,
            claude_signature_rate: item.claude_signature_rate,
            scenarios: item.scenarios,
            issues: item.issues,
        })
        .collect();
    RelayLeaderboard {
        items: apply_policy(parsed, config),
        ..cached
    }
}

fn write_cache(leaderboard: &RelayLeaderboard) {
    let path = cache_path(leaderboard.kind);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(leaderboard) {
        let _ = std::fs::write(path, bytes);
    }
}

fn prefer_live(
    live: Result<RelayLeaderboard, AppError>,
    cached: Option<RelayLeaderboard>,
) -> Result<RelayLeaderboard, AppError> {
    match live {
        Ok(leaderboard) => Ok(leaderboard),
        Err(error) => cached
            .map(|mut leaderboard| {
                leaderboard.from_cache = true;
                leaderboard
            })
            .ok_or(error),
    }
}

fn prefer_live_config(live: Option<RemoteConfig>, cached: Option<RemoteConfig>) -> RemoteConfig {
    live.or(cached).unwrap_or_default()
}

async fn fetch_page(kind: LeaderboardKind) -> Result<String, AppError> {
    use futures::StreamExt;
    let url = format!("{VERIDROP_ORIGIN}{}", kind.path());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("LoongPort/relay-directory")
        .build()
        .map_err(|error| AppError::Config(format!("创建 VeriDrop 请求失败: {error}")))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| AppError::Config(format!("拉取 VeriDrop 榜单失败: {error}")))?
        .error_for_status()
        .map_err(|error| AppError::Config(format!("VeriDrop 榜单响应失败: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PAGE_BYTES as u64)
    {
        return Err(AppError::Config("VeriDrop 榜单体积异常".into()));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(
            &chunk.map_err(|error| AppError::Config(format!("读取 VeriDrop 榜单失败: {error}")))?,
        );
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(AppError::Config("VeriDrop 榜单超过体积上限".into()));
        }
    }
    String::from_utf8(bytes)
        .map_err(|error| AppError::Config(format!("VeriDrop 榜单不是 UTF-8: {error}")))
}

pub async fn list(kind: LeaderboardKind) -> Result<RelayLeaderboard, AppError> {
    let cached_config = crate::relay::remote_config::load_cached();
    let config = prefer_live_config(
        crate::relay::remote_config::refresh_and_cache().await,
        cached_config,
    );
    let cached = read_cache(kind).map(|cached| apply_policy_to_cached(cached, &config));
    let live = fetch_page(kind).await.and_then(|html| {
        let parsed = parse_page(kind, &html, &managed_veridrop_hosts(&config))?;
        let items = apply_policy(parsed, &config);
        if items.is_empty() {
            return Err(AppError::Config("VeriDrop 榜单没有可展示站点".into()));
        }
        Ok(RelayLeaderboard {
            kind,
            items,
            synced_at: chrono::Utc::now().timestamp(),
            from_cache: false,
        })
    });
    if let Ok(leaderboard) = &live {
        write_cache(leaderboard);
    }
    prefer_live(live, cached)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_directory() -> RemoteConfig {
        RemoteConfig {
            relay_directory: RelayDirectoryPolicy {
                blocked_hosts: vec!["blocked.example".into()],
                sites: BTreeMap::from([
                    (
                        "790053500.com".into(),
                        crate::relay::remote_config::RelayDirectorySite {
                            veridrop_host: Some("api.790053500.com".into()),
                            entry_url: Some("https://790053500.com/keys".into()),
                            display_name: Some("鑫旺".into()),
                        },
                    ),
                    (
                        "blocked.example".into(),
                        crate::relay::remote_config::RelayDirectorySite {
                            veridrop_host: None,
                            entry_url: None,
                            display_name: None,
                        },
                    ),
                ]),
            },
            ..RemoteConfig::default()
        }
    }

    #[test]
    fn parse_certified_rows_and_required_metadata() {
        let page = parse_page(
            LeaderboardKind::Claude,
            include_str!("fixtures/veridrop-claude.html"),
            &["api.790053500.com".into()],
        )
        .expect("parse fixture");

        assert_eq!(page.len(), 2);
        assert_eq!(page[0].veridrop_host, "bestapi.store");
        assert_eq!(page[0].rank, 14);
        assert_eq!(page[0].score, 99);
        assert_eq!(page[0].samples, 20);
        assert_eq!(page[0].latest_date, "2026-08-12");
        assert_eq!(page[0].claude_signature_rate, Some(95));
        assert!(page[0]
            .scenarios
            .contains(&"Claude Code / Cursor 编程".to_string()));
        assert_eq!(page[1].veridrop_host, "api.790053500.com");
        assert_eq!(page[1].rank, 72, "managed site outside Top 60 is appended");
    }

    #[test]
    fn parse_excludes_official_and_unmanaged_non_top_rows() {
        let page = parse_page(
            LeaderboardKind::Claude,
            include_str!("fixtures/veridrop-claude.html"),
            &[],
        )
        .expect("parse fixture");

        assert_eq!(
            page.iter()
                .map(|item| item.veridrop_host.as_str())
                .collect::<Vec<_>>(),
            vec!["bestapi.store"]
        );
    }

    #[test]
    fn parse_supports_all_public_leaderboard_kinds() {
        let cases = [
            (
                LeaderboardKind::Overall,
                include_str!("fixtures/veridrop-overall.html"),
                "bestapi.store",
                95,
            ),
            (
                LeaderboardKind::OpenAi,
                include_str!("fixtures/veridrop-openai.html"),
                "bestapi.store",
                95,
            ),
            (
                LeaderboardKind::Gemini,
                include_str!("fixtures/veridrop-gemini.html"),
                "gemini.example",
                98,
            ),
        ];

        for (kind, fixture, host, score) in cases {
            let items = parse_page(kind, fixture, &[]).expect("parse fixture");
            assert_eq!(items.len(), 1, "{kind:?}");
            assert_eq!(items[0].veridrop_host, host, "{kind:?}");
            assert_eq!(items[0].score, score, "{kind:?}");
        }
    }

    #[test]
    fn leaderboard_kind_uses_frontend_tab_values() {
        let cases = [
            (LeaderboardKind::Overall, "\"overall\""),
            (LeaderboardKind::Claude, "\"claude\""),
            (LeaderboardKind::OpenAi, "\"openai\""),
            (LeaderboardKind::Gemini, "\"gemini\""),
        ];

        for (kind, json) in cases {
            assert_eq!(serde_json::to_string(&kind).unwrap(), json);
            assert_eq!(serde_json::from_str::<LeaderboardKind>(json).unwrap(), kind);
        }
    }

    #[test]
    fn policy_maps_alias_entry_and_display_name_and_blocks_incompatible_hosts() {
        let parsed = vec![
            ParsedLeaderboardItem {
                veridrop_host: "api.790053500.com".into(),
                rank: 72,
                score: 96,
                samples: 9,
                latest_date: "2026-08-11".into(),
                detail_url: "https://veridrop.org/leaderboard/api.790053500.com".into(),
                protocol_scores: vec![],
                claude_signature_rate: None,
                scenarios: vec![],
                issues: vec![],
            },
            ParsedLeaderboardItem {
                veridrop_host: "blocked.example".into(),
                rank: 1,
                score: 100,
                samples: 99,
                latest_date: "2026-08-13".into(),
                detail_url: "https://veridrop.org/leaderboard/blocked.example".into(),
                protocol_scores: vec![],
                claude_signature_rate: None,
                scenarios: vec![],
                issues: vec![],
            },
        ];

        let items = apply_policy(parsed, &config_with_directory());

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].site_host, "790053500.com");
        assert_eq!(items[0].veridrop_host, "api.790053500.com");
        assert_eq!(items[0].display_name, "鑫旺");
        assert_eq!(items[0].entry_url, "https://790053500.com/keys");
    }

    #[test]
    fn policy_keeps_only_the_first_ranked_row_for_each_site_identity() {
        let row = |veridrop_host: &str, rank: u32| ParsedLeaderboardItem {
            veridrop_host: veridrop_host.into(),
            rank,
            score: 96,
            samples: 9,
            latest_date: "2026-08-11".into(),
            detail_url: format!("https://veridrop.org/leaderboard/{veridrop_host}"),
            protocol_scores: vec![],
            claude_signature_rate: None,
            scenarios: vec![],
            issues: vec![],
        };

        let items = apply_policy(
            vec![row("api.790053500.com", 12), row("790053500.com", 72)],
            &config_with_directory(),
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].site_host, "790053500.com");
        assert_eq!(items[0].rank, 12);
    }

    #[test]
    fn live_result_wins_and_replaces_the_previous_cache() {
        let cached = RelayLeaderboard {
            kind: LeaderboardKind::Claude,
            items: vec![],
            synced_at: 1,
            from_cache: true,
        };
        let live = RelayLeaderboard {
            kind: LeaderboardKind::Claude,
            items: vec![],
            synced_at: 2,
            from_cache: false,
        };

        let selected = prefer_live(Ok(live.clone()), Some(cached)).unwrap();

        assert_eq!(selected, live);
        assert!(!selected.from_cache);
    }

    #[test]
    fn freshly_verified_directory_policy_wins_over_the_startup_cache() {
        let cached = RemoteConfig::default();
        let live = config_with_directory();

        let selected = prefer_live_config(Some(live.clone()), Some(cached));

        assert_eq!(selected, live);
    }

    #[test]
    fn live_failure_uses_the_last_successful_cache_and_marks_it() {
        let cached = RelayLeaderboard {
            kind: LeaderboardKind::Claude,
            items: vec![],
            synced_at: 1,
            from_cache: false,
        };

        let selected = prefer_live(Err(AppError::Config("offline".into())), Some(cached)).unwrap();

        assert!(selected.from_cache);
        assert_eq!(selected.synced_at, 1);
    }

    #[test]
    fn cached_items_are_filtered_by_the_current_signed_policy() {
        let cached = RelayLeaderboard {
            kind: LeaderboardKind::Claude,
            items: vec![RelayDirectoryItem {
                site_host: "blocked.example".into(),
                veridrop_host: "blocked.example".into(),
                display_name: "Blocked".into(),
                rank: 1,
                score: 100,
                samples: 99,
                latest_date: "2026-08-13".into(),
                detail_url: "https://veridrop.org/leaderboard/blocked.example".into(),
                protocol_scores: vec![],
                claude_signature_rate: None,
                scenarios: vec![],
                issues: vec![],
                entry_url: "https://blocked.example".into(),
            }],
            synced_at: 1,
            from_cache: true,
        };

        let filtered = apply_policy_to_cached(cached, &config_with_directory());

        assert!(filtered.items.is_empty());
    }

    #[tokio::test]
    #[ignore = "需要外网；手动跑 --ignored"]
    async fn live_public_pages_match_the_parser_contract() {
        for kind in [
            LeaderboardKind::Overall,
            LeaderboardKind::Claude,
            LeaderboardKind::OpenAi,
            LeaderboardKind::Gemini,
        ] {
            let html = fetch_page(kind).await.expect("fetch public leaderboard");
            let items = parse_page(kind, &html, &["api.790053500.com".into()])
                .expect("parse public leaderboard");
            assert!(!items.is_empty(), "{kind:?} should contain visible rows");
            assert!(
                items
                    .iter()
                    .any(|item| item.veridrop_host == "bestapi.store"),
                "{kind:?} should include the managed BestAPI site"
            );
            assert!(items.iter().all(|item| item.score <= 100));
        }
    }
}
