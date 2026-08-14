use crate::database::Database;
use crate::error::AppError;
use crate::maintenance::config::MODELS_DEV_REFRESH_INTERVAL;
use crate::services::model_pricing::{
    get_models_dev_sync_state, record_models_dev_sync_result, update_model_pricing_batch,
    ModelPricingInfo, ModelsDevSyncConfig,
};
use chrono::Utc;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const COMMON_MODEL_LIMIT_PER_FAMILY: usize = 6;
const NON_TEXT_MODEL_MARKERS: &[&str] = &[
    "audio",
    "deprecated",
    "embedding",
    "image",
    "moderation",
    "realtime",
    "transcribe",
    "tts",
    "video",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsDevEntry {
    pub key: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub normalized_id: String,
    pub model_name: String,
    pub release_date: String,
    pub input: String,
    pub output: String,
    pub cache_read: String,
    pub cache_write: String,
    pub is_common: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsDevSyncResult {
    pub skipped: bool,
    pub selected: usize,
    pub imported: usize,
    pub changed: usize,
    pub synced_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    name: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    name: Option<String>,
    release_date: Option<String>,
    cost: Option<ModelsDevCost>,
    modalities: Option<ModelsDevModalities>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevCost {
    input: Option<Box<RawValue>>,
    output: Option<Box<RawValue>>,
    cache_read: Option<Box<RawValue>>,
    cache_write: Option<Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    output: Vec<String>,
}

fn normalize_model_id(model_id: &str) -> String {
    let after_slash = model_id.rsplit('/').next().unwrap_or_default();
    let before_colon = after_slash.split(':').next().unwrap_or_default();
    let mut normalized = before_colon.trim().replace('@', "-").to_lowercase();
    if normalized.ends_with("[1m]") {
        normalized.truncate(normalized.len() - "[1m]".len());
        normalized = normalized.trim().to_string();
    }
    normalized
}

fn is_text_pricing_model(model_id: &str, model: &ModelsDevModel) -> bool {
    if model
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("deprecated"))
    {
        return false;
    }

    if let Some(modalities) = &model.modalities {
        if !modalities.output.is_empty() {
            let has_text = modalities
                .output
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("text"));
            let has_non_text = modalities.output.iter().any(|modality| {
                ["audio", "image", "video"]
                    .iter()
                    .any(|value| modality.eq_ignore_ascii_case(value))
            });
            if !has_text || has_non_text {
                return false;
            }
        }
    }

    let searchable_name =
        format!("{} {}", model_id, model.name.as_deref().unwrap_or_default()).to_lowercase();
    !NON_TEXT_MODEL_MARKERS
        .iter()
        .any(|marker| searchable_name.contains(marker))
}

fn is_json_number(value: Option<&RawValue>) -> bool {
    value.is_some_and(|value| {
        value
            .get()
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'-')
    })
}

fn price_string(value: Option<&RawValue>) -> String {
    let Some(number) = value.filter(|value| is_json_number(Some(value))) else {
        return "0".to_string();
    };
    let Ok(parsed) =
        Decimal::from_str(number.get()).or_else(|_| Decimal::from_scientific(number.get()))
    else {
        return "0".to_string();
    };
    if parsed <= Decimal::ZERO || parsed >= Decimal::from(1_000_000_000_000_u64) {
        return "0".to_string();
    }
    parsed
        .round_dp_with_strategy(6, RoundingStrategy::MidpointAwayFromZero)
        .normalize()
        .to_string()
}

fn common_family(provider_id: &str, model_id: &str) -> Option<&'static str> {
    let model_id = model_id.to_lowercase();
    match provider_id {
        "anthropic" if model_id.starts_with("claude-") => Some("claude"),
        "openai"
            if model_id.starts_with("gpt-")
                || model_id.starts_with("o1-")
                || model_id.starts_with("o3-")
                || model_id.starts_with("o4-") =>
        {
            Some("gpt")
        }
        "google" if model_id.starts_with("gemini-") => Some("gemini"),
        "xai" if model_id.starts_with("grok-") => Some("grok"),
        "deepseek" if model_id.starts_with("deepseek-") => Some("deepseek"),
        "alibaba" if model_id.starts_with("qwen") => Some("qwen"),
        "xiaomi" if model_id.starts_with("mimo-") => Some("mimo"),
        "longcat" if model_id.starts_with("longcat-") => Some("longcat"),
        "moonshotai" if model_id.starts_with("kimi-") => Some("kimi"),
        "minimax-cn" if model_id.starts_with("minimax-m") => Some("minimax"),
        "zai" if model_id.starts_with("glm-") => Some("glm"),
        _ => None,
    }
}

fn common_model_keys(entries: &[ModelsDevEntry]) -> BTreeSet<String> {
    let mut family_counts = BTreeMap::<&str, usize>::new();
    let mut keys = BTreeSet::new();
    for entry in entries {
        let Some(family) = common_family(&entry.provider_id, &entry.model_id) else {
            continue;
        };
        let count = family_counts.entry(family).or_default();
        if *count < COMMON_MODEL_LIMIT_PER_FAMILY {
            keys.insert(entry.key.clone());
            *count += 1;
        }
    }
    keys
}

fn parse_entries(json: &str) -> Result<Vec<ModelsDevEntry>, AppError> {
    let providers: BTreeMap<String, Option<ModelsDevProvider>> = serde_json::from_str(json)
        .map_err(|error| {
            AppError::Config(format!("models.dev catalog JSON is invalid: {error}"))
        })?;
    let mut entries = Vec::new();

    for (provider_id, provider) in providers {
        let Some(provider) = provider else {
            continue;
        };
        let provider_name = provider.name.unwrap_or_else(|| provider_id.clone());
        for (model_id, model) in provider.models {
            if !is_text_pricing_model(&model_id, &model) {
                continue;
            }
            let input = model.cost.as_ref().and_then(|cost| cost.input.as_deref());
            let output = model.cost.as_ref().and_then(|cost| cost.output.as_deref());
            if !is_json_number(input) && !is_json_number(output) {
                continue;
            }
            let normalized_id = normalize_model_id(&model_id);
            if normalized_id.is_empty() {
                continue;
            }
            let model_name = model.name.clone().unwrap_or_else(|| model_id.clone());
            entries.push(ModelsDevEntry {
                key: format!("{provider_id}/{model_id}"),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
                model_id,
                normalized_id,
                model_name,
                release_date: model.release_date.unwrap_or_default(),
                input: price_string(input),
                output: price_string(output),
                cache_read: price_string(
                    model
                        .cost
                        .as_ref()
                        .and_then(|cost| cost.cache_read.as_deref()),
                ),
                cache_write: price_string(
                    model
                        .cost
                        .as_ref()
                        .and_then(|cost| cost.cache_write.as_deref()),
                ),
                is_common: false,
            });
        }
    }

    entries.sort_by(|left, right| {
        right
            .release_date
            .cmp(&left.release_date)
            .then_with(|| left.model_name.cmp(&right.model_name))
    });
    let common = common_model_keys(&entries);
    for entry in &mut entries {
        entry.is_common = common.contains(&entry.key);
    }
    Ok(entries)
}

fn resolve_selection<'a>(
    entries: &'a [ModelsDevEntry],
    config: &ModelsDevSyncConfig,
) -> Vec<&'a ModelsDevEntry> {
    let explicit = config.selected_model_keys.iter().collect::<BTreeSet<_>>();
    let excluded = config
        .excluded_common_model_keys
        .iter()
        .collect::<BTreeSet<_>>();
    entries
        .iter()
        .filter(|entry| {
            explicit.contains(&entry.key)
                || (config.include_common_models
                    && entry.is_common
                    && !excluded.contains(&entry.key))
        })
        .collect()
}

fn to_model_pricing(entries: &[ModelsDevEntry]) -> Vec<ModelPricingInfo> {
    let mut normalized_ids = BTreeSet::new();
    entries
        .iter()
        .filter(|entry| normalized_ids.insert(entry.normalized_id.clone()))
        .map(|entry| ModelPricingInfo {
            model_id: entry.normalized_id.clone(),
            display_name: entry.model_name.clone(),
            input_cost_per_million: entry.input.clone(),
            output_cost_per_million: entry.output.clone(),
            cache_read_cost_per_million: entry.cache_read.clone(),
            cache_creation_cost_per_million: entry.cache_write.clone(),
        })
        .collect()
}

pub async fn fetch_entries() -> Result<Vec<ModelsDevEntry>, AppError> {
    let client = reqwest::Client::builder()
        .timeout(MODELS_DEV_FETCH_TIMEOUT)
        .build()
        .map_err(|error| {
            AppError::Message(format!("failed to build models.dev client: {error}"))
        })?;
    let response = client
        .get(MODELS_DEV_API_URL)
        .send()
        .await
        .map_err(|error| {
            AppError::Message(format!("failed to fetch models.dev catalog: {error}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::HttpStatus {
            status: status.as_u16(),
            body,
        });
    }
    let body = response.text().await.map_err(|error| {
        AppError::Message(format!("failed to read models.dev catalog: {error}"))
    })?;
    parse_entries(&body)
}

fn skipped_result(last_sync_at: Option<i64>) -> ModelsDevSyncResult {
    ModelsDevSyncResult {
        skipped: true,
        selected: 0,
        imported: 0,
        changed: 0,
        synced_at: last_sync_at,
    }
}

fn was_recently_synced(last_sync_at: Option<i64>, now: i64) -> bool {
    let Some(last_sync_at) = last_sync_at else {
        return false;
    };
    let refresh_interval_ms = MODELS_DEV_REFRESH_INTERVAL.as_millis() as i64;
    now.saturating_sub(last_sync_at) < refresh_interval_ms
}

async fn sync_pricing_with_fetch<F, Fut>(
    db: Arc<Database>,
    force: bool,
    fetch: F,
) -> Result<ModelsDevSyncResult, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<ModelsDevEntry>, AppError>>,
{
    let attempt: Result<ModelsDevSyncResult, AppError> = async {
        let initial = get_models_dev_sync_state(&db)?.config;
        let now = Utc::now().timestamp_millis();
        if !force && (!initial.auto_sync_enabled || was_recently_synced(initial.last_sync_at, now))
        {
            return Ok(skipped_result(initial.last_sync_at));
        }

        let entries = fetch().await?;
        let latest = get_models_dev_sync_state(&db)?.config;
        if !force && !latest.auto_sync_enabled {
            return Ok(skipped_result(latest.last_sync_at));
        }

        let selected = resolve_selection(&entries, &latest);
        let selected_count = selected.len();
        let selected = selected.into_iter().cloned().collect::<Vec<_>>();
        let pricing = to_model_pricing(&selected);
        let imported = pricing.len();
        let changed = update_model_pricing_batch(&db, pricing)?;
        let synced_at = Utc::now().timestamp_millis();
        record_models_dev_sync_result(&db, Some(synced_at), None)?;

        Ok(ModelsDevSyncResult {
            skipped: false,
            selected: selected_count,
            imported,
            changed,
            synced_at: Some(synced_at),
        })
    }
    .await;

    if let Err(error) = &attempt {
        if let Err(record_error) = record_models_dev_sync_result(&db, None, Some(error.to_string()))
        {
            log::warn!("failed to record models.dev sync error: {record_error}");
        }
    }
    attempt
}

pub async fn sync_pricing(db: Arc<Database>, force: bool) -> Result<ModelsDevSyncResult, AppError> {
    sync_pricing_with_fetch(db, force, fetch_entries).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::model_pricing::save_models_dev_sync_config;
    use serial_test::serial;
    use std::ffi::OsString;

    struct TestHome {
        _temp: tempfile::TempDir,
        previous: Option<OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
            Self {
                _temp: temp,
                previous,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn sync_config(
        auto_sync_enabled: bool,
        selected_model_keys: Vec<&str>,
        last_sync_at: Option<i64>,
    ) -> ModelsDevSyncConfig {
        ModelsDevSyncConfig {
            auto_sync_enabled,
            include_common_models: false,
            selected_model_keys: selected_model_keys
                .into_iter()
                .map(str::to_string)
                .collect(),
            excluded_common_model_keys: Vec::new(),
            last_sync_at,
            last_sync_error: None,
        }
    }

    fn fixture_entries() -> Vec<ModelsDevEntry> {
        parse_entries(include_str!("fixtures/models-dev-sample.json")).expect("parse fixture")
    }

    #[test]
    fn catalog_filters_non_text_models_and_normalizes_ids() {
        let entries = fixture_entries();

        assert!(entries.iter().any(|entry| entry.normalized_id == "gpt-5"));
        assert!(entries
            .iter()
            .any(|entry| entry.normalized_id == "claude-sonnet-4-5"));
        assert!(!entries
            .iter()
            .any(|entry| entry.model_id.contains("embedding")));
        assert!(!entries.iter().any(|entry| entry.model_id.contains("image")));
        assert!(!entries
            .iter()
            .any(|entry| entry.model_id.contains("speech")));
        assert!(!entries
            .iter()
            .any(|entry| entry.model_id.contains("legacy")));
    }

    #[test]
    fn catalog_strips_provider_and_model_prefixes_and_sanitizes_prices() {
        let entries = fixture_entries();
        let relay = entries
            .iter()
            .find(|entry| entry.key == "relay/vendor/GPT-5")
            .expect("relay entry");
        assert_eq!(relay.normalized_id, "gpt-5");

        let invalid = entries
            .iter()
            .find(|entry| entry.key == "relay/invalid-price-model")
            .expect("invalid price entry");
        assert_eq!(invalid.input, "0");
        assert_eq!(invalid.output, "0");
        assert_eq!(invalid.cache_read, "0");
        assert_eq!(invalid.cache_write, "0");
    }

    #[test]
    fn catalog_preserves_decimal_precision_before_six_place_rounding() {
        let entries = parse_entries(
            r#"{
                "exact": {
                    "models": {
                        "exact-price": {
                            "cost": { "input": 999999999999.123456, "output": 1 }
                        }
                    }
                }
            }"#,
        )
        .expect("parse exact decimal");

        assert_eq!(entries[0].input, "999999999999.123456");
    }

    #[test]
    fn selection_combines_common_and_explicit_models() {
        let entries = fixture_entries();
        let config = ModelsDevSyncConfig {
            auto_sync_enabled: true,
            include_common_models: true,
            selected_model_keys: vec!["relay/custom-model".into()],
            excluded_common_model_keys: Vec::new(),
            last_sync_at: None,
            last_sync_error: None,
        };

        let selected = resolve_selection(&entries, &config);
        assert_eq!(
            selected
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec![
                "anthropic/claude-sonnet-4-5[1m]",
                "openai/gpt-5",
                "relay/custom-model"
            ]
        );
    }

    #[test]
    fn pricing_deduplicates_normalized_ids_in_catalog_order() {
        let entries = fixture_entries();
        let pricing = to_model_pricing(&entries);
        let gpt = pricing
            .iter()
            .filter(|entry| entry.model_id == "gpt-5")
            .collect::<Vec<_>>();

        assert_eq!(gpt.len(), 1);
        assert_eq!(gpt[0].display_name, "GPT-5");
        assert_eq!(gpt[0].input_cost_per_million, "1.25");
    }

    #[test]
    fn common_selection_is_limited_to_six_recent_models_per_family() {
        let json = serde_json::json!({
            "openai": {
                "name": "OpenAI",
                "models": (1..=7)
                    .map(|version| (
                        format!("gpt-{version}"),
                        serde_json::json!({
                            "name": format!("GPT {version}"),
                            "release_date": format!("2025-0{version}-01"),
                            "cost": { "input": version, "output": version * 2 }
                        }),
                    ))
                    .collect::<serde_json::Map<_, _>>()
            },
            "aggregator": {
                "models": {
                    "gpt-8": {
                        "release_date": "2026-01-01",
                        "cost": { "input": 8, "output": 16 }
                    }
                }
            }
        });
        let entries = parse_entries(&json.to_string()).expect("parse common models");
        let common = common_model_keys(&entries);

        assert_eq!(common.len(), 6);
        assert!(common.contains("openai/gpt-7"));
        assert!(!common.contains("openai/gpt-1"));
        assert!(!common.contains("aggregator/gpt-8"));
    }

    #[tokio::test]
    #[serial]
    async fn automatic_sync_skips_when_disabled() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        save_models_dev_sync_config(&db, sync_config(false, vec!["relay/custom-model"], None))
            .expect("save disabled config");

        let result = sync_pricing_with_fetch(db, false, || async {
            panic!("disabled automatic sync must not fetch")
        })
        .await
        .expect("skip disabled sync");

        assert!(result.skipped);
        assert_eq!(result.selected, 0);
        assert_eq!(result.imported, 0);
        assert_eq!(result.changed, 0);
        assert_eq!(result.synced_at, None);
    }

    #[tokio::test]
    #[serial]
    async fn automatic_sync_skips_when_last_success_is_recent() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        let recent = chrono::Utc::now().timestamp_millis();
        save_models_dev_sync_config(
            &db,
            sync_config(true, vec!["relay/custom-model"], Some(recent)),
        )
        .expect("save recent config");

        let result = sync_pricing_with_fetch(db, false, || async {
            panic!("recent automatic sync must not fetch")
        })
        .await
        .expect("skip recent sync");

        assert!(result.skipped);
        assert_eq!(result.synced_at, Some(recent));
    }

    #[tokio::test]
    #[serial]
    async fn forced_sync_runs_when_automatic_sync_is_disabled() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        save_models_dev_sync_config(&db, sync_config(false, vec!["relay/custom-model"], None))
            .expect("save disabled config");

        let result =
            sync_pricing_with_fetch(Arc::clone(&db), true, || async { Ok(fixture_entries()) })
                .await
                .expect("force sync");

        assert!(!result.skipped);
        assert_eq!(result.selected, 1);
        assert_eq!(result.imported, 1);
        assert_eq!(result.changed, 1);
        assert!(result.synced_at.is_some());
        let custom_models: i64 = db
            .conn
            .lock()
            .expect("lock database")
            .query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'custom-model'",
                [],
                |row| row.get(0),
            )
            .expect("query imported model");
        assert_eq!(custom_models, 1);
    }

    #[tokio::test]
    #[serial]
    async fn automatic_sync_aborts_when_user_disables_during_download() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        save_models_dev_sync_config(&db, sync_config(true, vec!["relay/custom-model"], None))
            .expect("save enabled config");
        let db_during_fetch = Arc::clone(&db);

        let result = sync_pricing_with_fetch(Arc::clone(&db), false, move || async move {
            save_models_dev_sync_config(
                &db_during_fetch,
                sync_config(false, vec!["relay/custom-model"], None),
            )
            .expect("disable during download");
            Ok(fixture_entries())
        })
        .await
        .expect("abort disabled sync");

        assert!(result.skipped);
        let custom_models: i64 = db
            .conn
            .lock()
            .expect("lock database")
            .query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'custom-model'",
                [],
                |row| row.get(0),
            )
            .expect("query skipped model");
        assert_eq!(custom_models, 0);
    }

    #[tokio::test]
    #[serial]
    async fn sync_uses_selection_reloaded_after_download() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        save_models_dev_sync_config(&db, sync_config(true, vec!["relay/custom-model"], None))
            .expect("save initial selection");
        let db_during_fetch = Arc::clone(&db);

        let result = sync_pricing_with_fetch(Arc::clone(&db), false, move || async move {
            save_models_dev_sync_config(
                &db_during_fetch,
                sync_config(true, vec!["openai/gpt-5"], None),
            )
            .expect("change selection during download");
            Ok(fixture_entries())
        })
        .await
        .expect("sync latest selection");

        assert_eq!(result.selected, 1);
        let conn = db.conn.lock().expect("lock database");
        let custom_models: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'custom-model'",
                [],
                |row| row.get(0),
            )
            .expect("query stale selection");
        let gpt_input: String = conn
            .query_row(
                "SELECT input_cost_per_million FROM model_pricing WHERE model_id = 'gpt-5'",
                [],
                |row| row.get(0),
            )
            .expect("query latest selection");
        assert_eq!(custom_models, 0);
        assert_eq!(gpt_input, "1.25");
    }

    #[tokio::test]
    #[serial]
    async fn failed_sync_records_error_without_replacing_last_success() {
        let _home = TestHome::new();
        let db = Arc::new(Database::memory().expect("memory database"));
        let previous_success = 1_700_000_000_000;
        save_models_dev_sync_config(
            &db,
            sync_config(true, vec!["relay/custom-model"], Some(previous_success)),
        )
        .expect("save previous success");

        let error = sync_pricing_with_fetch(Arc::clone(&db), false, || async {
            Err(AppError::Message("catalog offline".to_string()))
        })
        .await
        .expect_err("sync should fail");
        assert!(error.to_string().contains("catalog offline"));

        let state = get_models_dev_sync_state(&db).expect("reload sync result");
        assert_eq!(state.config.last_sync_at, Some(previous_success));
        assert_eq!(
            state.config.last_sync_error.as_deref(),
            Some("catalog offline")
        );
    }
}
