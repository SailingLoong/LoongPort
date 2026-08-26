//! 上传器：把闭合小时桶 flush 到 Worker ingest 端点。
//!
//! 水位线（`crowd_metrics_flushed_through`，存 DB `settings` 表）只在**成功后**推进：
//! 失败下个周期从原水位线重算重发 —— 天然幂等重试，不需要队列。
//! 客户端对同一小时总是重发**全量**桶（本地现算），服务端 INSERT OR REPLACE 覆盖。
//!
//! 端点在 Worker 部署之前 DNS 解析不到 —— 失败静默、水位线不动，无害；
//! 发布顺序见 crowd-metrics/README（先部署 Worker 再发客户端版本）。

use std::time::Duration;

use serde::Serialize;

use crate::crowd::bucket::{self, HourBucket};
use crate::database::Database;
use crate::error::AppError;

pub const INGEST_URL: &str = "https://metrics.loongport.dev/v1/ingest";

const FETCH_TIMEOUT_SECS: u64 = 10;
/// 首次开启（无水位线）只回溯这么多 —— 不翻旧账，也让首包小而快。
const COLD_START_WINDOW_SECS: i64 = 24 * 3600;
/// 服务端单次 POST 上限是 200 桶；客户端按更小的块切，留余量。
const CHUNK_SIZE: usize = 150;

const SETTING_FLUSHED_THROUGH: &str = "crowd_metrics_flushed_through";
const SETTING_SOURCE_DAY: &str = "crowd_metrics_source_day";
const SETTING_SOURCE_ID: &str = "crowd_metrics_source_id";

/// 一次上传载荷。字段集合被闸测试钉死 —— 加字段前先过模块文档那张表。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IngestPayload<'a> {
    pub version: i32,
    pub source_id: &'a str,
    pub hours: Vec<HourBucketPayload<'a>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HourBucketPayload<'a> {
    pub hour: String,
    pub site: &'a str,
    pub app: &'a str,
    pub samples: i64,
    pub errors: i64,
    pub ttft_bins: &'a [i64],
    pub ttft_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd_micros: i64,
}

fn payload_from_bucket<'a>(source_id: &'a str, buckets: &'a [HourBucket]) -> IngestPayload<'a> {
    IngestPayload {
        version: 1,
        source_id,
        hours: buckets
            .iter()
            .map(|b| HourBucketPayload {
                hour: hour_string(b.hour_epoch),
                site: &b.site,
                app: &b.app,
                samples: b.samples,
                errors: b.errors,
                ttft_bins: &b.ttft_bins,
                ttft_count: b.ttft_count,
                input_tokens: b.input_tokens,
                output_tokens: b.output_tokens,
                cache_read_tokens: b.cache_read_tokens,
                cache_creation_tokens: b.cache_creation_tokens,
                cost_usd_micros: b.cost_usd_micros,
            })
            .collect(),
    }
}

/// epoch（小时整点）→ Worker 约定的 `YYYY-MM-DDTHHZ`（UTC）。
pub fn hour_string(hour_epoch: i64) -> String {
    chrono::DateTime::from_timestamp(hour_epoch, 0)
        .expect("小时整点必然落在 chrono 可表示范围内")
        .format("%Y-%m-%dT%HZ")
        .to_string()
}

/// 当日 source id：同一天复用（跨重启稳定、重传可去重），隔日即换。
fn ensure_daily_source_id(db: &Database, now_epoch: i64) -> Result<String, AppError> {
    let day = now_epoch / 86400;
    if db.get_setting(SETTING_SOURCE_DAY)?.as_deref() == Some(day.to_string().as_str()) {
        if let Some(id) = db.get_setting(SETTING_SOURCE_ID)? {
            if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(id);
            }
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    db.set_setting(SETTING_SOURCE_DAY, &day.to_string())?;
    db.set_setting(SETTING_SOURCE_ID, &id)?;
    Ok(id)
}

/// flush 一轮：门禁 → 算桶 → 上传 → 推水位线。由 maintenance 调度器周期调用。
///
/// 收 `&Arc<Database>`：阻塞 DB 工作要搬进 `spawn_blocking`（'static），
/// 引用进不去 —— 与 `run_session_sync` 同一个形态。
pub async fn flush_once(db: &std::sync::Arc<Database>) -> Result<(), AppError> {
    // 共建门禁：设置里关着就一个字节都不发（连读都省）。
    if !crate::settings::get_settings().crowd_metrics_enabled {
        return Ok(());
    }

    let now = chrono::Utc::now().timestamp();
    let last_closed_hour = (now / 3600 - 1) * 3600;

    let db_for_state = std::sync::Arc::clone(db);
    let (after, source_id) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(i64, String), AppError> {
            let flushed_through = db_for_state
                .get_setting(SETTING_FLUSHED_THROUGH)?
                .and_then(|v| v.parse::<i64>().ok());
            let after = match flushed_through {
                Some(prev) => prev + 3600,
                None => (last_closed_hour - COLD_START_WINDOW_SECS + 3600).max(0),
            };
            let source_id = ensure_daily_source_id(&db_for_state, now)?;
            Ok((after, source_id))
        })
        .await
        .map_err(|e| AppError::Message(format!("crowd flush 状态读取任务失败: {e}")))??;

    if after > last_closed_hour {
        return Ok(()); // 水位线已到最新闭合小时
    }

    let db_for_buckets = std::sync::Arc::clone(db);
    let buckets = tauri::async_runtime::spawn_blocking(move || {
        bucket::build_hour_buckets(&db_for_buckets, after, last_closed_hour + 3600)
    })
    .await
    .map_err(|e| AppError::Message(format!("crowd flush 分桶任务失败: {e}")))??;

    if !buckets.is_empty() {
        send(&source_id, &buckets).await?;
    }

    // 走到这里 = 本轮数据已上传（或本来就没有）。无论哪种都推进水位线，
    // 否则空转周期会反复重算同一段空窗口。
    let db_for_watermark = std::sync::Arc::clone(db);
    tauri::async_runtime::spawn_blocking(move || {
        db_for_watermark.set_setting(SETTING_FLUSHED_THROUGH, &last_closed_hour.to_string())
    })
    .await
    .map_err(|e| AppError::Message(format!("crowd flush 水位线写入任务失败: {e}")))??;
    Ok(())
}

async fn send(source_id: &str, buckets: &[HourBucket]) -> Result<(), AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Config(format!("构造 crowd 上传客户端失败: {e}")))?;

    for chunk in buckets.chunks(CHUNK_SIZE) {
        let payload = payload_from_bucket(source_id, chunk);
        let resp = client
            .post(INGEST_URL)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::Config(format!("crowd 上传失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Config(format!(
                "crowd 上传被拒: {}",
                resp.status()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bucket() -> HourBucket {
        HourBucket {
            hour_epoch: 1_756_218_000, // 2025-08-26T15Z 之类的整点，值本身由格式化断言钉住
            site: "example.com".to_string(),
            app: "claude".to_string(),
            samples: 10,
            errors: 1,
            ttft_bins: vec![0; crate::crowd::bins::TTFT_BIN_COUNT],
            ttft_count: 0,
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 300,
            cache_creation_tokens: 100,
            cost_usd_micros: 12_345,
        }
    }

    #[test]
    fn hour_string_is_utc_fixed_format() {
        // 2026-08-26T07:00:00Z = 1787724360? 直接用 from_timestamp 反推不直观，
        // 钉住一个已知值：0 = 1970-01-01T00Z。
        assert_eq!(hour_string(0), "1970-01-01T00Z");
        // 3600 * 24 * 365 = 1971-01-01T00Z（1970 无闰日）。
        assert_eq!(hour_string(31_536_000), "1971-01-01T00Z");
    }

    #[test]
    fn payload_field_set_is_pinned() {
        // ⭐ 本模块最重要的闸（照 relay::stats 的同款）：载荷键集合被钉死，
        // 将来加字段时这条当场红 —— 加字段前先回模块文档「传/不传」的表。
        let buckets = [sample_bucket()];
        let payload = payload_from_bucket("0123456789abcdef0123456789abcdef", &buckets);
        let json = serde_json::to_value(&payload).unwrap();

        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["hours", "sourceId", "version"]);

        let hour_json = &json["hours"][0];
        let mut hour_keys: Vec<&str> = hour_json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        hour_keys.sort_unstable();
        assert_eq!(
            hour_keys,
            vec![
                "app",
                "cacheCreationTokens",
                "cacheReadTokens",
                "costUsdMicros",
                "errors",
                "hour",
                "inputTokens",
                "outputTokens",
                "samples",
                "site",
                "ttftBins",
                "ttftCount",
            ],
            "小时桶的字段集合变了 —— 加字段前请回 crowd 模块文档那张表"
        );

        // 反面：身份/凭据形态一个都不许出现（判据同 relay::stats）。
        let text = json.to_string();
        for forbidden in [
            "sk-",
            "email",
            "@",
            "balance",
            "password",
            "refresh",
            "apikey",
            "api_key",
            "accountid",
            "account_id",
            "username",
            "nickname",
        ] {
            assert!(
                !text.to_ascii_lowercase().contains(forbidden),
                "载荷里出现了 {forbidden}: {text}"
            );
        }
    }

    #[test]
    fn source_id_rotates_daily_and_survives_restart() {
        let db = Database::memory().unwrap();
        let day = 20_000;
        let first = ensure_daily_source_id(&db, day * 86400 + 100).unwrap();
        // 同日复读（模拟重启后）：稳定。
        assert_eq!(
            ensure_daily_source_id(&db, day * 86400 + 200).unwrap(),
            first
        );
        // 隔日：换新。
        let next = ensure_daily_source_id(&db, (day + 1) * 86400).unwrap();
        assert_ne!(first, next);
        assert_eq!(first.len(), 32);
    }
}
