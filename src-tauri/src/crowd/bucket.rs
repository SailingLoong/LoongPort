//! 从 `proxy_request_logs` 切出上传用的小时聚合桶。
//!
//! 三段式，各自可测：
//! 1. [`query_raw_buckets`] —— SQL 按 `(hour, provider, app)` 切桶（provider 维度）；
//! 2. [`resolve_relay_hosts`] —— 薄胶水：provider 指纹 → 归一 host，只留 relay 模块
//!    登记过的站点（v1 边界，见模块文档）；
//! 3. [`merge_by_site`] —— 纯函数：按 `(hour, site, app)` 合并（同站多账号一桶，
//!    与服务端幂等键同粒度）。
//!
//! 桶的合并单位是 `(hour, site, app)`，客户端对同一小时总是重发**全量**桶
//! （本地现算），服务端 INSERT OR REPLACE 覆盖 —— 天然幂等。

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::params;

use crate::crowd::bins::{ttft_bin_sum_exprs, TTFT_BIN_COUNT};
use crate::database::Database;
use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;

/// 一个待上传的小时聚合桶。字段集合就是上传载荷的字段集合 ——
/// 加字段前先回模块文档那张「传/不传」的表过一遍。
#[derive(Debug, Clone, PartialEq)]
pub struct HourBucket {
    /// 小时起点（unix 秒，UTC 整点）。
    pub hour_epoch: i64,
    /// 归一化站点 host（小写、无 scheme/端口/www）。
    pub site: String,
    /// app 标识（`app_type` 原样）。
    pub app: String,
    pub samples: i64,
    /// 失败请求数（status < 200 或 ≥ 400，含网络错误的 0）。
    pub errors: i64,
    /// TTFT 直方图计数，长度恒为 [`TTFT_BIN_COUNT`]。
    pub ttft_bins: Vec<i64>,
    /// 有 `first_token_ms` 的样本数（= `ttft_bins` 求和）。
    pub ttft_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// 桶内总花费（微美元）。
    pub cost_usd_micros: i64,
}

/// SQL 切出的 provider 维度桶（站点归属尚未解析）。
#[derive(Debug, Clone)]
struct RawBucket {
    hour_epoch: i64,
    provider_id: String,
    app_type: String,
    samples: i64,
    errors: i64,
    ttft_bins: Vec<i64>,
    ttft_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd_micros: i64,
}

/// 查询并切桶（provider 维度）。`after_epoch`（不含）到 `before_epoch`（含）限定行窗口；
/// 只取 `data_source = 'proxy'` 的行（session 回填行时间戳是同步时间，见模块文档）。
fn query_raw_buckets(
    db: &Database,
    after_epoch: i64,
    before_epoch: i64,
) -> Result<Vec<RawBucket>, AppError> {
    let bins_expr = ttft_bin_sum_exprs("l");
    let sql = format!(
        "SELECT CAST(l.created_at / 3600 AS INTEGER) * 3600 AS hour_epoch, \
                l.provider_id, l.app_type, \
                COUNT(*), \
                SUM(CASE WHEN l.status_code < 200 OR l.status_code >= 400 THEN 1 ELSE 0 END), \
                {bins_expr}, \
                SUM(CASE WHEN l.first_token_ms IS NOT NULL THEN 1 ELSE 0 END), \
                SUM({fresh_input}), \
                SUM(l.output_tokens), SUM(l.cache_read_tokens), SUM(l.cache_creation_tokens), \
                CAST(ROUND(SUM(CAST(l.total_cost_usd AS REAL)) * 1000000.0) AS INTEGER) \
         FROM proxy_request_logs l \
         WHERE l.data_source = 'proxy' AND l.created_at > ?1 AND l.created_at <= ?2 \
         GROUP BY hour_epoch, l.provider_id, l.app_type",
        fresh_input = fresh_input_sql("l"),
    );

    let conn = crate::database::lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query(params![after_epoch, before_epoch])
        .map_err(|e| AppError::Database(e.to_string()))?;

    let mut raw_buckets = Vec::new();
    while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        let mut bins = Vec::with_capacity(TTFT_BIN_COUNT);
        for i in 0..TTFT_BIN_COUNT {
            bins.push(row.get::<_, i64>(5 + i)?);
        }
        raw_buckets.push(RawBucket {
            hour_epoch: row.get(0)?,
            provider_id: row.get(1)?,
            app_type: row.get(2)?,
            samples: row.get(3)?,
            errors: row.get(4)?,
            ttft_bins: bins,
            ttft_count: row.get(5 + TTFT_BIN_COUNT)?,
            input_tokens: row.get(6 + TTFT_BIN_COUNT)?,
            output_tokens: row.get(7 + TTFT_BIN_COUNT)?,
            cache_read_tokens: row.get(8 + TTFT_BIN_COUNT)?,
            cache_creation_tokens: row.get(9 + TTFT_BIN_COUNT)?,
            cost_usd_micros: row.get(10 + TTFT_BIN_COUNT)?,
        });
    }
    Ok(raw_buckets)
}

/// provider → 归一 host，只保留 relay 模块登记过的站点。
///
/// 判据：provider 的 base_url 指纹归一成 host 后，命中 `loongport_relay` 表里
/// 任一站点的归一 host。托管档（`loongport-` 前缀）创建时必写 creds 行，
/// 所以这一条规则同时覆盖托管与手填档，不需要第二条特判。
fn resolve_relay_hosts(
    db: &Database,
    refs: &HashSet<(String, String)>,
) -> Result<HashMap<(String, String), String>, AppError> {
    let relay_hosts: HashSet<String> = {
        let conn = crate::database::lock_conn!(db.conn);
        crate::relay::creds::list(&conn)?
            .into_iter()
            .map(|relay| crate::relay::aff::lookup_host(&relay.site_origin))
            .collect()
    };
    if relay_hosts.is_empty() {
        return Ok(HashMap::new());
    }

    let mut hosts = HashMap::new();
    let mut app_types: HashSet<&str> = refs.iter().map(|(_, app)| app.as_str()).collect();
    for app_str in app_types.drain() {
        let Ok(app_type) = app_str.parse::<crate::app_config::AppType>() else {
            continue;
        };
        let providers = db.get_all_providers(app_str)?;
        for (provider_id, provider) in &providers {
            if !refs.contains(&(provider_id.clone(), app_str.to_string())) {
                continue;
            }
            let Some((origin, _api_key)) =
                crate::relay::provider_fingerprint::for_provider(provider, &app_type)
            else {
                continue;
            };
            let host = crate::relay::aff::lookup_host(&origin);
            if relay_hosts.contains(&host) {
                hosts.insert((provider_id.clone(), app_str.to_string()), host);
            }
        }
    }
    Ok(hosts)
}

/// 纯函数：按 `(hour, site, app)` 合并。host 映射里没有的 provider 直接丢弃。
fn merge_by_site(
    raws: Vec<RawBucket>,
    hosts: &HashMap<(String, String), String>,
) -> Vec<HourBucket> {
    let mut merged: BTreeMap<(i64, String, String), HourBucket> = BTreeMap::new();
    for raw in raws {
        let Some(site) = hosts.get(&(raw.provider_id.clone(), raw.app_type.clone())) else {
            continue;
        };
        let key = (raw.hour_epoch, site.clone(), raw.app_type.clone());
        let entry = merged.entry(key).or_insert_with(|| HourBucket {
            hour_epoch: raw.hour_epoch,
            site: site.clone(),
            app: raw.app_type.clone(),
            samples: 0,
            errors: 0,
            ttft_bins: vec![0; TTFT_BIN_COUNT],
            ttft_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd_micros: 0,
        });
        entry.samples += raw.samples;
        entry.errors += raw.errors;
        for (i, count) in raw.ttft_bins.iter().enumerate() {
            entry.ttft_bins[i] += count;
        }
        entry.ttft_count += raw.ttft_count;
        entry.input_tokens += raw.input_tokens;
        entry.output_tokens += raw.output_tokens;
        entry.cache_read_tokens += raw.cache_read_tokens;
        entry.cache_creation_tokens += raw.cache_creation_tokens;
        entry.cost_usd_micros += raw.cost_usd_micros;
    }
    merged.into_values().collect()
}

/// 组合入口：查桶 → 解析站点归属 → 合并。由 [`super::uploader`] 调用。
pub fn build_hour_buckets(
    db: &Database,
    after_epoch: i64,
    before_epoch: i64,
) -> Result<Vec<HourBucket>, AppError> {
    let raws = query_raw_buckets(db, after_epoch, before_epoch)?;
    if raws.is_empty() {
        return Ok(Vec::new());
    }
    let refs: HashSet<(String, String)> = raws
        .iter()
        .map(|raw| (raw.provider_id.clone(), raw.app_type.clone()))
        .collect();
    let hosts = resolve_relay_hosts(db, &refs)?;
    Ok(merge_by_site(raws, &hosts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        // `Database::memory()` 已按生产 schema 建齐全部表 —— 这里不再自建
        // （自建会撞「table already exists」，且形状迟早与生产漂移）。
        Database::memory().expect("内存库")
    }

    #[allow(clippy::too_many_arguments)] // 测试播种器：一列一参，比构造器结构直白
    fn seed_log(
        db: &Database,
        id: &str,
        provider: &str,
        app: &str,
        status: i64,
        first_token_ms: Option<i64>,
        cost: &str,
        at: i64,
        data_source: &str,
    ) {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, status_code, first_token_ms,
                    total_cost_usd, latency_ms, created_at, data_source
                 ) VALUES (?1, ?2, ?3, 'test-model', ?4, ?5, ?6, 0, ?7, ?8)",
                params![
                    id,
                    provider,
                    app,
                    status,
                    first_token_ms,
                    cost,
                    at,
                    data_source
                ],
            )
            .unwrap();
    }

    #[test]
    fn query_buckets_by_hour_and_skip_session_rows() {
        let db = setup_db();
        // 11:00 与 12:00 各两条 + 一条 session 回填（必须被忽略）。
        seed_log(
            &db,
            "a",
            "p1",
            "claude",
            200,
            Some(250),
            "0.5",
            11 * 3600 + 100,
            "proxy",
        );
        seed_log(
            &db,
            "b",
            "p1",
            "claude",
            500,
            None,
            "0",
            11 * 3600 + 200,
            "proxy",
        );
        seed_log(
            &db,
            "c",
            "p1",
            "claude",
            200,
            Some(700),
            "1.5",
            12 * 3600 + 300,
            "proxy",
        );
        seed_log(
            &db,
            "d",
            "p2",
            "claude",
            200,
            None,
            "0",
            12 * 3600 + 400,
            "proxy",
        );
        seed_log(
            &db,
            "sess",
            "p1",
            "claude",
            200,
            Some(100),
            "9",
            12 * 3600 + 500,
            "session_log",
        );

        let raws = query_raw_buckets(&db, 0, 13 * 3600).unwrap();
        assert_eq!(raws.len(), 3, "两小时 × (p1, p2) 分桶，session 行不计");

        let h11_p1 = raws
            .iter()
            .find(|r| r.hour_epoch == 11 * 3600 && r.provider_id == "p1")
            .expect("11 点 p1 桶存在");
        assert_eq!(h11_p1.samples, 2);
        assert_eq!(h11_p1.errors, 1);
        assert_eq!(h11_p1.ttft_count, 1);
        assert_eq!(h11_p1.ttft_bins[1], 1, "250ms 落 [200,400) 桶");
        assert_eq!(h11_p1.cost_usd_micros, 500_000);

        let h12_p1 = raws
            .iter()
            .find(|r| r.hour_epoch == 12 * 3600 && r.provider_id == "p1")
            .expect("12 点 p1 桶存在");
        assert_eq!(h12_p1.samples, 1);
        assert_eq!(h12_p1.ttft_bins[3], 1, "700ms 落 [600,800) 桶");
    }

    #[test]
    fn query_window_bounds_are_exclusive_after_inclusive_before() {
        let db = setup_db();
        seed_log(
            &db,
            "edge-low",
            "p1",
            "claude",
            200,
            None,
            "0",
            10 * 3600,
            "proxy",
        );
        seed_log(
            &db,
            "in",
            "p1",
            "claude",
            200,
            None,
            "0",
            10 * 3600 + 1,
            "proxy",
        );
        seed_log(
            &db,
            "edge-high",
            "p1",
            "claude",
            200,
            None,
            "0",
            11 * 3600,
            "proxy",
        );

        let raws = query_raw_buckets(&db, 10 * 3600, 11 * 3600).unwrap();
        // 两行分属 10 点与 11 点桶，各 1 行；edge-low（恰在 after 上）被排除，
        // edge-high（恰在 before 上）被包含。
        assert_eq!(raws.len(), 2, "after 不含、before 含");
        let by_hour: HashMap<i64, i64> = raws
            .into_iter()
            .map(|r| (r.hour_epoch, r.samples))
            .collect();
        assert_eq!(by_hour[&(10 * 3600)], 1);
        assert_eq!(by_hour[&(11 * 3600)], 1);
    }

    #[test]
    fn merge_by_site_joins_accounts_and_drops_unknown_providers() {
        let db = setup_db();
        seed_log(
            &db,
            "a",
            "acct-a",
            "claude",
            200,
            Some(250),
            "0.5",
            11 * 3600 + 100,
            "proxy",
        );
        seed_log(
            &db,
            "b",
            "acct-b",
            "claude",
            200,
            Some(550),
            "1.5",
            11 * 3600 + 200,
            "proxy",
        );
        seed_log(
            &db,
            "c",
            "official",
            "claude",
            200,
            None,
            "0",
            11 * 3600 + 300,
            "proxy",
        );

        let raws = query_raw_buckets(&db, 0, 12 * 3600).unwrap();
        let mut hosts = HashMap::new();
        hosts.insert(
            ("acct-a".to_string(), "claude".to_string()),
            "example.com".to_string(),
        );
        hosts.insert(
            ("acct-b".to_string(), "claude".to_string()),
            "example.com".to_string(),
        );

        let merged = merge_by_site(raws, &hosts);
        assert_eq!(
            merged.len(),
            1,
            "同站两账号合并成一桶，未登记 provider 丢弃"
        );
        let bucket = &merged[0];
        assert_eq!(bucket.site, "example.com");
        assert_eq!(bucket.samples, 2);
        assert_eq!(bucket.cost_usd_micros, 2_000_000);
        assert_eq!(bucket.ttft_count, 2);
    }
}
