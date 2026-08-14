use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::error::AppError;
use crate::maintenance::config::VERIDROP_CACHE_TTL;
use crate::relay::remote_config::{RelayDirectoryPolicy, RemoteConfig};

const VERIDROP_ORIGIN: &str = "https://veridrop.org";
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 12;
const MANAGED_DETAIL_CONCURRENCY: usize = 4;
const CACHE_SCHEMA_VERSION: u8 = 1;

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
    pub rank: Option<u32>,
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
    pub rank: Option<u32>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedLeaderboard {
    schema_version: u8,
    kind: LeaderboardKind,
    items: Vec<ParsedLeaderboardItem>,
    synced_at: i64,
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

    fn protocol_name(self) -> Option<&'static str> {
        match self {
            Self::Overall => None,
            Self::Claude => Some("Claude"),
            Self::OpenAi => Some("OpenAI"),
            Self::Gemini => Some("Gemini"),
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
            rank: Some(rank),
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

fn parse_detail_page(
    kind: LeaderboardKind,
    host: &str,
    html: &str,
) -> Result<Option<ParsedLeaderboardItem>, AppError> {
    let document = Html::parse_document(html);
    let score_selector = selector(".lb-detail-score-num")?;
    let protocol_selector = selector(".lb-detail-protocols .lb-proto-chip")?;
    let protocol_name_selector = selector(".lb-proto-name")?;
    let protocol_score_selector = selector(".lb-proto-score")?;
    let protocol_count_selector = selector(".lb-proto-count")?;
    let protocol_verdict_selector = selector(".lb-proto-verdict")?;
    let signature_selector = selector(".lb-proto-sig")?;
    let issue_selector = selector(".lb-detail-issues code")?;
    let summary_selector = selector(".answer-capsule")?;
    let latest_date_pattern =
        regex::Regex::new(r"最近一次检测[:：]\s*(\d{4}-\d{2}-\d{2})").expect("static regex");
    let overall_samples_pattern =
        regex::Regex::new(r"累计\s*(\d+)\s*次独立检测").expect("static regex");

    let mut protocol_scores = Vec::new();
    let mut claude_signature_rate = None;
    for protocol in document.select(&protocol_selector) {
        let Some(name) = protocol.select(&protocol_name_selector).next().map(text) else {
            continue;
        };
        let Some(score) = protocol
            .select(&protocol_score_selector)
            .next()
            .and_then(|element| parse_score(&text(element)))
            .filter(|score| *score <= 100)
        else {
            continue;
        };
        let samples = protocol
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
            score,
            samples,
            verdict,
            report_url,
        });
    }

    let (score, samples) = match kind.protocol_name() {
        Some(protocol_name) => {
            let Some(protocol) = protocol_scores
                .iter()
                .find(|protocol| protocol.protocol == protocol_name)
            else {
                return Ok(None);
            };
            (protocol.score, protocol.samples)
        }
        None => {
            let Some(score) = document
                .select(&score_selector)
                .next()
                .and_then(|element| parse_score(&text(element)))
                .filter(|score| *score <= 100)
            else {
                return Ok(None);
            };
            let summary = document
                .select(&summary_selector)
                .next()
                .map(text)
                .unwrap_or_default();
            let samples = overall_samples_pattern
                .captures(&summary)
                .and_then(|captures| captures.get(1))
                .and_then(|value| value.as_str().parse().ok())
                .unwrap_or(0);
            (score, samples)
        }
    };
    let summary = document
        .select(&summary_selector)
        .next()
        .map(text)
        .unwrap_or_default();
    let latest_date = latest_date_pattern
        .captures(&summary)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
        .unwrap_or_default();
    let host = crate::relay::aff::lookup_host(host);
    if host.is_empty() {
        return Ok(None);
    }

    Ok(Some(ParsedLeaderboardItem {
        detail_url: format!("{VERIDROP_ORIGIN}/leaderboard/{host}"),
        veridrop_host: host,
        rank: None,
        score,
        samples,
        latest_date,
        protocol_scores,
        claude_signature_rate,
        scenarios: vec![],
        issues: document.select(&issue_selector).map(text).collect(),
    }))
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
    let mut blocked: BTreeSet<_> = policy.blocked_hosts.iter().cloned().collect();
    let mut aliases = BTreeMap::new();
    for (loongport_host, site) in &policy.sites {
        let veridrop_host = site
            .veridrop_host
            .as_deref()
            .map(crate::relay::aff::lookup_host)
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| loongport_host.clone());
        aliases.insert(loongport_host.clone(), veridrop_host.clone());
        aliases.insert(veridrop_host.clone(), veridrop_host.clone());
        if blocked.contains(loongport_host) || blocked.contains(&veridrop_host) {
            blocked.insert(loongport_host.clone());
            blocked.insert(veridrop_host);
        }
    }

    let mut candidates = BTreeSet::new();
    for sponsor in &config.sponsors {
        candidates.insert(crate::relay::aff::lookup_host(&sponsor.site_origin));
    }
    candidates.extend(
        config
            .aff_codes
            .keys()
            .map(|host| crate::relay::aff::lookup_host(host)),
    );
    candidates.extend(
        config
            .promo_codes
            .keys()
            .map(|host| crate::relay::aff::lookup_host(host)),
    );
    for (loongport_host, site) in &policy.sites {
        candidates.insert(
            site.veridrop_host
                .as_deref()
                .map(crate::relay::aff::lookup_host)
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| loongport_host.clone()),
        );
    }
    candidates
        .into_iter()
        .filter(|host| !host.is_empty() && !blocked.contains(host))
        .map(|host| aliases.get(&host).cloned().unwrap_or(host))
        .filter(|host| !blocked.contains(host))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

fn read_cache(kind: LeaderboardKind) -> Option<CachedLeaderboard> {
    let path = cache_path(kind);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_PAGE_BYTES as u64 {
        return None;
    }
    let cached: CachedLeaderboard = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION || cached.kind != kind {
        return None;
    }
    Some(cached)
}

fn apply_policy_to_cached(cached: CachedLeaderboard, config: &RemoteConfig) -> RelayLeaderboard {
    RelayLeaderboard {
        kind: cached.kind,
        items: apply_policy(cached.items, config),
        synced_at: cached.synced_at,
    }
}

fn write_cache(leaderboard: &CachedLeaderboard) -> Result<(), AppError> {
    let path = cache_path(leaderboard.kind);
    let bytes = serde_json::to_vec(leaderboard)
        .map_err(|source| AppError::JsonSerialize { source })?;
    crate::config::atomic_write(&path, &bytes)
}

fn is_fresh_at(synced_at: i64, now: i64) -> bool {
    now.saturating_sub(synced_at) < VERIDROP_CACHE_TTL.as_secs() as i64
}

pub fn read_cached(kind: LeaderboardKind) -> Result<Option<RelayLeaderboard>, AppError> {
    let config = crate::relay::remote_config::load_cached().unwrap_or_default();
    Ok(read_cache(kind).map(|cached| apply_policy_to_cached(cached, &config)))
}

pub fn is_cache_fresh(kind: LeaderboardKind, now: i64) -> bool {
    read_cache(kind).is_some_and(|cached| is_fresh_at(cached.synced_at, now))
}

async fn fetch_html(client: &reqwest::Client, url: &str) -> Result<Option<String>, AppError> {
    use futures::StreamExt;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| AppError::Config(format!("拉取 VeriDrop 页面失败: {error}")))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| AppError::Config(format!("VeriDrop 页面响应失败: {error}")))?;
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
        .map(Some)
        .map_err(|error| AppError::Config(format!("VeriDrop 页面不是 UTF-8: {error}")))
}

async fn fetch_live_source_with(
    client: &reqwest::Client,
    origin: &str,
    kind: LeaderboardKind,
    managed_hosts: &[String],
) -> Result<Vec<ParsedLeaderboardItem>, AppError> {
    use futures::StreamExt;

    let leaderboard_url = format!("{origin}{}", kind.path());
    let html = fetch_html(client, &leaderboard_url)
        .await?
        .ok_or_else(|| AppError::Config("VeriDrop 榜单不存在".into()))?;
    let mut items = parse_page(kind, &html, managed_hosts)?;
    let present: BTreeSet<_> = items
        .iter()
        .map(|item| item.veridrop_host.clone())
        .collect();
    let missing: Vec<_> = managed_hosts
        .iter()
        .filter(|host| !present.contains(*host))
        .cloned()
        .collect();
    let mut details = futures::stream::iter(missing.into_iter().map(|host| async move {
        let detail_url = format!("{origin}/leaderboard/{host}");
        let Some(html) = fetch_html(client, &detail_url).await? else {
            return Ok(None);
        };
        parse_detail_page(kind, &host, &html)
    }))
    .buffered(MANAGED_DETAIL_CONCURRENCY);
    while let Some(detail) = details.next().await {
        if let Some(item) = detail? {
            items.push(item);
        }
    }
    Ok(items)
}

async fn fetch_live_source(
    kind: LeaderboardKind,
    managed_hosts: &[String],
) -> Result<Vec<ParsedLeaderboardItem>, AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("LoongPort/relay-directory")
        .build()
        .map_err(|error| AppError::Config(format!("创建 VeriDrop 请求失败: {error}")))?;
    fetch_live_source_with(&client, VERIDROP_ORIGIN, kind, managed_hosts).await
}

static OVERALL_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CLAUDE_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static OPENAI_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static GEMINI_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn refresh_lock(kind: LeaderboardKind) -> &'static tokio::sync::Mutex<()> {
    match kind {
        LeaderboardKind::Overall => &OVERALL_REFRESH_LOCK,
        LeaderboardKind::Claude => &CLAUDE_REFRESH_LOCK,
        LeaderboardKind::OpenAi => &OPENAI_REFRESH_LOCK,
        LeaderboardKind::Gemini => &GEMINI_REFRESH_LOCK,
    }
}

async fn refresh_with(
    client: &reqwest::Client,
    origin: &str,
    kind: LeaderboardKind,
    config: &RemoteConfig,
) -> Result<RelayLeaderboard, AppError> {
    let parsed = fetch_live_source_with(client, origin, kind, &managed_veridrop_hosts(config)).await?;
    let items = apply_policy(parsed.clone(), config);
    if items.is_empty() {
        return Err(AppError::Config("VeriDrop 榜单没有可展示站点".into()));
    }
    let synced_at = chrono::Utc::now().timestamp();
    write_cache(&CachedLeaderboard {
        schema_version: CACHE_SCHEMA_VERSION,
        kind,
        items: parsed,
        synced_at,
    })?;
    Ok(RelayLeaderboard {
        kind,
        items,
        synced_at,
    })
}

pub async fn refresh(kind: LeaderboardKind) -> Result<RelayLeaderboard, AppError> {
    let previous_synced_at = read_cache(kind).map(|cached| cached.synced_at);
    let _guard = refresh_lock(kind).lock().await;
    if let Some(cached) = read_cache(kind) {
        let refreshed_while_waiting = previous_synced_at
            .map(|previous| cached.synced_at > previous)
            .unwrap_or(true);
        if refreshed_while_waiting {
            let config = crate::relay::remote_config::load_cached().unwrap_or_default();
            return Ok(apply_policy_to_cached(cached, &config));
        }
    }

    let config = crate::relay::remote_config::load_cached().unwrap_or_default();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("LoongPort/relay-directory")
        .build()
        .map_err(|error| AppError::Config(format!("创建 VeriDrop 请求失败: {error}")))?;
    refresh_with(&client, VERIDROP_ORIGIN, kind, &config).await
}

pub async fn refresh_if_stale(kind: LeaderboardKind) -> Result<RelayLeaderboard, AppError> {
    let now = chrono::Utc::now().timestamp();
    if is_cache_fresh(kind, now) {
        if let Some(cached) = read_cached(kind)? {
            return Ok(cached);
        }
    }
    refresh(kind).await
}

pub async fn list(kind: LeaderboardKind) -> Result<RelayLeaderboard, AppError> {
    if let Some(cached) = read_cached(kind)? {
        return Ok(cached);
    }
    refresh(kind).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{response::Html, routing::get, Router};
    use serial_test::serial;

    struct TestHomeGuard(Option<std::ffi::OsString>);

    impl TestHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", path);
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
        assert_eq!(page[0].rank, Some(14));
        assert_eq!(page[0].score, 99);
        assert_eq!(page[0].samples, 20);
        assert_eq!(page[0].latest_date, "2026-08-12");
        assert_eq!(page[0].claude_signature_rate, Some(95));
        assert!(page[0]
            .scenarios
            .contains(&"Claude Code / Cursor 编程".to_string()));
        assert_eq!(page[1].veridrop_host, "api.790053500.com");
        assert_eq!(
            page[1].rank,
            Some(72),
            "managed site outside Top 60 is appended"
        );
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

    #[tokio::test]
    async fn fetches_a_managed_detail_when_the_selected_leaderboard_omits_it() {
        let app = Router::new()
            .route(
                "/leaderboard/gemini",
                get(|| async { Html(include_str!("fixtures/veridrop-gemini.html")) }),
            )
            .route(
                "/leaderboard/wawapii.com",
                get(|| async { Html(include_str!("fixtures/veridrop-detail-wawapii.html")) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local VeriDrop fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve VeriDrop fixtures");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("fixture client");

        let items = fetch_live_source_with(
            &client,
            &format!("http://{address}"),
            LeaderboardKind::Gemini,
            &["wawapii.com".into()],
        )
        .await
        .expect("fetch leaderboard with managed details");
        server.abort();

        let managed = items
            .iter()
            .find(|item| item.veridrop_host == "wawapii.com")
            .expect("managed site with a Gemini score is appended");
        assert_eq!(managed.rank, None, "detail pages do not own a rank");
        assert_eq!(managed.score, 100);
        assert_eq!(managed.samples, 1);
        assert_eq!(managed.latest_date, "2026-08-13");
        assert_eq!(managed.protocol_scores.len(), 3);
        assert_eq!(managed.issues, vec!["token_usage"]);
    }

    #[tokio::test]
    async fn managed_details_keep_the_configured_order_when_responses_finish_out_of_order() {
        let app = Router::new()
            .route(
                "/leaderboard/gemini",
                get(|| async { Html(include_str!("fixtures/veridrop-gemini.html")) }),
            )
            .route(
                "/leaderboard/slow.example",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Html(include_str!("fixtures/veridrop-detail-wawapii.html"))
                }),
            )
            .route(
                "/leaderboard/fast.example",
                get(|| async { Html(include_str!("fixtures/veridrop-detail-wawapii.html")) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local VeriDrop fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve VeriDrop fixtures");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("fixture client");

        let items = fetch_live_source_with(
            &client,
            &format!("http://{address}"),
            LeaderboardKind::Gemini,
            &["slow.example".into(), "fast.example".into()],
        )
        .await
        .expect("fetch ordered managed details");
        server.abort();

        assert_eq!(
            items
                .iter()
                .filter(|item| item.rank.is_none())
                .map(|item| item.veridrop_host.as_str())
                .collect::<Vec<_>>(),
            vec!["slow.example", "fast.example"]
        );
    }

    #[tokio::test]
    async fn blocked_site_identity_does_not_fetch_its_veridrop_alias() {
        let app = Router::new()
            .route(
                "/leaderboard/gemini",
                get(|| async { Html(include_str!("fixtures/veridrop-gemini.html")) }),
            )
            .route(
                "/leaderboard/api.790053500.com",
                get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local VeriDrop fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve VeriDrop fixtures");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("fixture client");
        let mut config = config_with_directory();
        config
            .aff_codes
            .insert("790053500.com".into(), "invite".into());
        config.relay_directory.blocked_hosts = vec!["790053500.com".into()];

        let items = fetch_live_source_with(
            &client,
            &format!("http://{address}"),
            LeaderboardKind::Gemini,
            &managed_veridrop_hosts(&config),
        )
        .await
        .expect("blocked alias must not make the leaderboard fail");
        server.abort();

        assert_eq!(
            items
                .iter()
                .map(|item| item.veridrop_host.as_str())
                .collect::<Vec<_>>(),
            vec!["gemini.example"]
        );
    }

    #[test]
    fn detail_samples_do_not_include_digits_from_the_host_name() {
        let html = include_str!("fixtures/veridrop-detail-wawapii.html")
            .replace("wawapii.com", "790053500.com");

        let item = parse_detail_page(LeaderboardKind::Overall, "790053500.com", &html)
            .expect("parse detail fixture")
            .expect("overall score is present");

        assert_eq!(item.samples, 276);
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
                rank: Some(72),
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
                rank: Some(1),
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
            rank: Some(rank),
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
        assert_eq!(items[0].rank, Some(12));
    }

    #[test]
    fn cache_is_fresh_until_exactly_six_hours() {
        let synced_at = 1_786_680_000;

        assert!(is_fresh_at(synced_at, synced_at + 6 * 60 * 60 - 1));
        assert!(!is_fresh_at(synced_at, synced_at + 6 * 60 * 60));
    }

    #[test]
    fn relay_leaderboard_serialization_has_no_from_cache_mirror() {
        let leaderboard = RelayLeaderboard {
            kind: LeaderboardKind::Claude,
            items: vec![],
            synced_at: 1_786_680_000,
        };

        let value = serde_json::to_value(leaderboard).expect("serialize leaderboard");

        assert!(value.get("fromCache").is_none());
        assert_eq!(value["syncedAt"], 1_786_680_000);
    }

    #[tokio::test]
    #[serial]
    async fn failed_refresh_preserves_previous_cache_bytes_and_timestamp() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _home = TestHomeGuard::set(temp.path());
        let previous = CachedLeaderboard {
            schema_version: CACHE_SCHEMA_VERSION,
            kind: LeaderboardKind::Claude,
            items: vec![ParsedLeaderboardItem {
                veridrop_host: "bestapi.store".into(),
                rank: Some(1),
                score: 99,
                samples: 20,
                latest_date: "2026-08-12".into(),
                detail_url: "https://veridrop.org/leaderboard/bestapi.store".into(),
                protocol_scores: vec![],
                claude_signature_rate: None,
                scenarios: vec![],
                issues: vec![],
            }],
            synced_at: 1_786_680_000,
        };
        write_cache(&previous).expect("write previous cache");
        let path = cache_path(LeaderboardKind::Claude);
        let previous_bytes = std::fs::read(&path).expect("read previous cache");

        let app = Router::new().route(
            "/leaderboard/claude",
            get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local VeriDrop fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve VeriDrop fixtures");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("fixture client");

        let result = refresh_with(
            &client,
            &format!("http://{address}"),
            LeaderboardKind::Claude,
            &RemoteConfig::default(),
        )
        .await;
        server.abort();

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).expect("read preserved cache"),
            previous_bytes
        );
        assert_eq!(
            read_cache(LeaderboardKind::Claude)
                .expect("preserved cache")
                .synced_at,
            previous.synced_at
        );
    }

    #[test]
    fn managed_hosts_use_the_policy_owned_veridrop_identity_once() {
        let mut config = config_with_directory();
        config
            .aff_codes
            .insert("790053500.com".into(), "invite".into());

        assert_eq!(managed_veridrop_hosts(&config), vec!["api.790053500.com"]);
    }

    #[test]
    fn cached_items_are_filtered_by_the_current_signed_policy() {
        let cached = CachedLeaderboard {
            schema_version: CACHE_SCHEMA_VERSION,
            kind: LeaderboardKind::Claude,
            items: vec![ParsedLeaderboardItem {
                veridrop_host: "blocked.example".into(),
                rank: Some(1),
                score: 100,
                samples: 99,
                latest_date: "2026-08-13".into(),
                detail_url: "https://veridrop.org/leaderboard/blocked.example".into(),
                protocol_scores: vec![],
                claude_signature_rate: None,
                scenarios: vec![],
                issues: vec![],
            }],
            synced_at: 1,
        };

        let filtered = apply_policy_to_cached(cached, &config_with_directory());

        assert!(filtered.items.is_empty());
    }

    #[test]
    fn cached_veridrop_facts_can_be_restored_after_a_site_is_unblocked() {
        let source = ParsedLeaderboardItem {
            veridrop_host: "blocked.example".into(),
            rank: Some(1),
            score: 100,
            samples: 99,
            latest_date: "2026-08-13".into(),
            detail_url: "https://veridrop.org/leaderboard/blocked.example".into(),
            protocol_scores: vec![],
            claude_signature_rate: None,
            scenarios: vec![],
            issues: vec![],
        };
        let cached: CachedLeaderboard = serde_json::from_slice(
            &serde_json::to_vec(&CachedLeaderboard {
                schema_version: CACHE_SCHEMA_VERSION,
                kind: LeaderboardKind::Claude,
                items: vec![source],
                synced_at: 1,
            })
            .expect("serialize current cache shape"),
        )
        .expect("read current cache shape");

        let restored = apply_policy_to_cached(cached, &RemoteConfig::default());

        assert_eq!(restored.items.len(), 1);
        assert_eq!(restored.items[0].site_host, "blocked.example");
    }

    #[test]
    fn legacy_policy_filtered_cache_is_not_accepted_as_source_facts() {
        let legacy = serde_json::json!({
            "kind": "claude",
            "items": [{
                "siteHost": "bestapi.store",
                "veridropHost": "bestapi.store",
                "displayName": "BestAPI",
                "rank": 1,
                "score": 99,
                "samples": 20,
                "latestDate": "2026-08-12",
                "detailUrl": "https://veridrop.org/leaderboard/bestapi.store",
                "protocolScores": [],
                "claudeSignatureRate": null,
                "scenarios": [],
                "issues": [],
                "entryUrl": "https://bestapi.store"
            }],
            "syncedAt": 1,
            "fromCache": true
        });

        assert!(serde_json::from_value::<CachedLeaderboard>(legacy).is_err());
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
            let items = fetch_live_source(kind, &["api.790053500.com".into()])
                .await
                .expect("parse public leaderboard and managed details");
            assert!(!items.is_empty(), "{kind:?} should contain visible rows");
            assert!(
                items
                    .iter()
                    .any(|item| item.veridrop_host == "bestapi.store"),
                "{kind:?} should include the managed BestAPI site"
            );
            assert!(items.iter().all(|item| item.score <= 100));
        }

        let gemini = fetch_live_source(LeaderboardKind::Gemini, &["wawapii.com".into()])
            .await
            .expect("fetch Gemini leaderboard and managed detail");
        let wawapi = gemini
            .iter()
            .find(|item| item.veridrop_host == "wawapii.com")
            .expect("WawAPI should be completed from its public detail page");
        assert_eq!(wawapi.rank, None);
        assert!(wawapi
            .protocol_scores
            .iter()
            .any(|protocol| protocol.protocol == "Gemini"));
    }
}
