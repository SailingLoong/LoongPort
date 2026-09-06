//! 从 `proxy_request_logs` 切出上传用的小时聚合桶。
//!
//! 三段式，各自可测：
//! 1. [`query_raw_buckets`] —— SQL 按 `(hour, provider, app)` 切桶（provider 维度）；
//! 2. [`resolve_relay_hosts`] —— 薄胶水：provider 指纹 → 站点身份（注册域），
//!    只留 relay 模块登记过的站点（v1 边界，见模块文档）；
//! 3. [`merge_by_site`] —— 纯函数：按 `(hour, site, app)` 合并（同站多账号一桶，
//!    与服务端幂等键同粒度）。
//!
//! 桶的合并单位是 `(hour, site, app)`，客户端对同一小时总是重发**全量**桶
//! （本地现算），服务端 INSERT OR REPLACE 覆盖 —— 天然幂等。

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::params;

use crate::crowd::bins::{tps_bin_sum_exprs, ttft_bin_sum_exprs, TPS_BIN_COUNT, TTFT_BIN_COUNT};
use crate::database::Database;
use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;

/// `errors` 的口径：**站点侧失败** —— 失败行里剔除「不是站点的锅」的三类：
///
/// - 401/402：凭证/余额级，是上传者自己的账号问题（与转发侧
///   `is_fatal_upstream_error` 的致命分级同一语义），不该抬站点的公开错误率；
/// - 403 且 body 不含站点侧标记：凭证级 403。body 命中「余额不足/上游」的是
///   newapi 家族把**站点侧**故障包成的 403（标记与 forwarder 的
///   `SITE_SIDE_403_MARKERS` 同源），照算站点的锅；
/// - 503 且是本机未出门的错误（无可用 Provider / 全部熔断 / 未配置）：
///   请求根本没到站点。
///
/// 两个跨文件事实由单元测试钉住：403 的 LIKE 必须锚定落库文案前缀
/// 「上游错误 (403): 」（前缀本身就含「上游」二字，裸 LIKE 会把所有上游
/// 403 都判成站点侧）；503 的三条本机文案与 `error_mapper::get_error_message`
/// 逐字一致 —— 测试用真实 mapper 输出播种，任一侧改文案当场红。
const SITE_SIDE_ERROR_EXPR: &str = "(l.status_code < 200 OR l.status_code >= 400) \
     AND NOT ( \
         l.status_code IN (401, 402) \
         OR (l.status_code = 403 \
             AND COALESCE(l.error_message, '') NOT LIKE '上游错误 (403): %余额不足%' \
             AND COALESCE(l.error_message, '') NOT LIKE '上游错误 (403): %上游%') \
         OR (l.status_code = 503 AND COALESCE(l.error_message, '') IN ( \
             '无可用 Provider', '所有供应商已熔断，无可用渠道', '未配置供应商')) \
     )";

/// 一个待上传的小时聚合桶。字段集合就是上传载荷的字段集合 ——
/// 加字段前先回模块文档那张「传/不传」的表过一遍。
#[derive(Debug, Clone, PartialEq)]
pub struct HourBucket {
    /// 小时起点（unix 秒，UTC 整点）。
    pub hour_epoch: i64,
    /// 站点身份：注册域（apex）。上传后就是快照的站点键。
    pub site: String,
    /// app 标识（`app_type` 原样）。
    pub app: String,
    pub samples: i64,
    /// 站点侧失败请求数（口径见 [`SITE_SIDE_ERROR_EXPR`]：凭证/余额级与本机
    /// 未出门的失败不计 —— 它们不反映站点健康，混进去会把上传者自己的
    /// 账号问题变成全站的公开错误率）。
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
    /// P4 模型维度：桶内按模型的子聚合（按模型名排序，载荷字节稳定）。
    /// 顶层字段仍是全量口径 —— 站点级聚合/旧消费方不受影响。
    pub models: Vec<ModelBucket>,
    /// P4b：该小时该站的非致命熔断跳闸次数（事件表计数；口径见 events.rs）。
    pub breaker_trips: i64,
}

/// P4：模型维度的子聚合（站点 × app × 小时 × 模型）。
/// 字段是 HourBucket 的子集 + tps 直方图 —— 供网站模型筛选/斩杀线图阵。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBucket {
    /// 落库 `model` 原样（服务端模型名，公开目录名，无身份信息）。
    pub model: String,
    pub samples: i64,
    pub errors: i64,
    pub ttft_bins: Vec<i64>,
    /// 输出速度直方图（tok/s，边界见 bins.rs `TPS_BIN_EDGES`），长度恒 [`TPS_BIN_COUNT`]。
    pub tps_bins: Vec<i64>,
    pub output_tokens: i64,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd_micros: i64,
    /// P5：被动观察到的模型真伪异常次数（Anomaly 级事件计数，事件源见
    /// `crowd::events::record_model_anomaly`）。恒 ≤ `samples`（异常响应
    /// 必然是被计数的请求之一）。
    pub anomalies: i64,
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

/// P4：模型维度的 provider 桶（站点归属尚未解析）。
#[derive(Debug, Clone)]
struct RawModelBucket {
    hour_epoch: i64,
    provider_id: String,
    app_type: String,
    model: String,
    samples: i64,
    errors: i64,
    ttft_bins: Vec<i64>,
    tps_bins: Vec<i64>,
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
                SUM(CASE WHEN {site_side_error} THEN 1 ELSE 0 END), \
                {bins_expr}, \
                SUM(CASE WHEN l.first_token_ms IS NOT NULL THEN 1 ELSE 0 END), \
                SUM({fresh_input}), \
                SUM(l.output_tokens), SUM(l.cache_read_tokens), SUM(l.cache_creation_tokens), \
                CAST(ROUND(SUM(CAST(l.total_cost_usd AS REAL)) * 1000000.0) AS INTEGER) \
         FROM proxy_request_logs l \
         WHERE l.data_source = 'proxy' AND l.created_at > ?1 AND l.created_at <= ?2 \
         GROUP BY hour_epoch, l.provider_id, l.app_type",
        fresh_input = fresh_input_sql("l"),
        site_side_error = SITE_SIDE_ERROR_EXPR,
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

/// P4：模型维度切桶 —— 与 [`query_raw_buckets`] 同窗口同口径，只是多按
/// `model` 分组并带 TPS 直方图。两次查询分开走（UNION ALL 会把 SQL 撑得
/// 不可读，且模型行的列集不同）。
fn query_raw_model_buckets(
    db: &Database,
    after_epoch: i64,
    before_epoch: i64,
) -> Result<Vec<RawModelBucket>, AppError> {
    let ttft_exprs = ttft_bin_sum_exprs("l");
    let tps_exprs = tps_bin_sum_exprs("l");
    let sql = format!(
        "SELECT CAST(l.created_at / 3600 AS INTEGER) * 3600 AS hour_epoch, \
                l.provider_id, l.app_type, l.model, \
                COUNT(*), \
                SUM(CASE WHEN {site_side_error} THEN 1 ELSE 0 END), \
                {ttft_exprs}, \
                {tps_exprs}, \
                SUM({fresh_input}), \
                SUM(l.output_tokens), SUM(l.cache_read_tokens), SUM(l.cache_creation_tokens), \
                CAST(ROUND(SUM(CAST(l.total_cost_usd AS REAL)) * 1000000.0) AS INTEGER) \
         FROM proxy_request_logs l \
         WHERE l.data_source = 'proxy' AND l.created_at > ?1 AND l.created_at <= ?2 \
         GROUP BY hour_epoch, l.provider_id, l.app_type, l.model",
        fresh_input = fresh_input_sql("l"),
        site_side_error = SITE_SIDE_ERROR_EXPR,
    );

    let conn = crate::database::lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query(params![after_epoch, before_epoch])
        .map_err(|e| AppError::Database(e.to_string()))?;

    let mut raws = Vec::new();
    while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        let mut ttft_bins = Vec::with_capacity(TTFT_BIN_COUNT);
        for i in 0..TTFT_BIN_COUNT {
            ttft_bins.push(row.get::<_, i64>(5 + i)?);
        }
        let mut tps_bins = Vec::with_capacity(TPS_BIN_COUNT);
        for i in 0..TPS_BIN_COUNT {
            tps_bins.push(row.get::<_, i64>(5 + TTFT_BIN_COUNT + i)?);
        }
        let base = 5 + TTFT_BIN_COUNT + TPS_BIN_COUNT;
        raws.push(RawModelBucket {
            hour_epoch: row.get(0)?,
            provider_id: row.get(1)?,
            app_type: row.get(2)?,
            model: row.get(3)?,
            samples: row.get(4)?,
            errors: row.get(5)?,
            ttft_bins,
            tps_bins,
            input_tokens: row.get(base)?,
            output_tokens: row.get(base + 1)?,
            cache_read_tokens: row.get(base + 2)?,
            cache_creation_tokens: row.get(base + 3)?,
            cost_usd_micros: row.get(base + 4)?,
        });
    }
    Ok(raws)
}

/// provider → 站点身份（注册域），只保留 relay 模块登记过的站点。
///
/// 判据：provider 的 base_url 指纹归到注册域后，命中 `loongport_relay` 表里任一
/// 站点的注册域。站点的**两列**都算数（`site_origin` 面板域 + `api_base_url` API 域
/// ——同一个站常分挂不同子域，2026-09-05 前只按 site_origin 的全 host 匹配，
/// `api.` 子域全部静默失配、整桶丢弃）。托管档（`loongport-` 前缀）创建时必写
/// creds 行，所以这一条规则同时覆盖托管与手填档，不需要第二条特判。
fn resolve_relay_hosts(
    db: &Database,
    refs: &HashSet<(String, String)>,
) -> Result<HashMap<(String, String), String>, AppError> {
    let relay_domains: HashSet<String> = {
        let conn = crate::database::lock_conn!(db.conn);
        crate::relay::creds::list(&conn)?
            .into_iter()
            .flat_map(|relay| {
                [
                    crate::relay::identity::site_domain(&relay.site_origin),
                    crate::relay::identity::site_domain(&relay.api_base_url),
                ]
            })
            .collect()
    };
    if relay_domains.is_empty() {
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
            let domain = crate::relay::identity::site_domain(&origin);
            if relay_domains.contains(&domain) {
                hosts.insert((provider_id.clone(), app_str.to_string()), domain);
            }
        }
    }
    Ok(hosts)
}

/// 纯函数：按 `(hour, site, app)` 合并。host 映射里没有的 provider 直接丢弃。
/// P4：模型子桶按 `(hour, site, app, model)` 同步合并进 `HourBucket.models`。
fn merge_by_site(
    raws: Vec<RawBucket>,
    model_raws: Vec<RawModelBucket>,
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
            models: Vec::new(),
            breaker_trips: 0,
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

    // 模型子桶：并进对应 HourBucket（键必然已存在 —— 模型行来自同一查询窗口，
    // 顶层桶先合并完；万一站点级桶被丢弃，模型行同样丢弃，不产生孤儿）。
    let mut models: BTreeMap<(i64, String, String, String), ModelBucket> = BTreeMap::new();
    for raw in model_raws {
        let Some(site) = hosts.get(&(raw.provider_id.clone(), raw.app_type.clone())) else {
            continue;
        };
        let key = (
            raw.hour_epoch,
            site.clone(),
            raw.app_type.clone(),
            raw.model.clone(),
        );
        let entry = models.entry(key).or_insert_with(|| ModelBucket {
            model: raw.model.clone(),
            samples: 0,
            errors: 0,
            ttft_bins: vec![0; TTFT_BIN_COUNT],
            tps_bins: vec![0; TPS_BIN_COUNT],
            output_tokens: 0,
            input_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd_micros: 0,
            anomalies: 0,
        });
        entry.samples += raw.samples;
        entry.errors += raw.errors;
        for (i, count) in raw.ttft_bins.iter().enumerate() {
            entry.ttft_bins[i] += count;
        }
        for (i, count) in raw.tps_bins.iter().enumerate() {
            entry.tps_bins[i] += count;
        }
        entry.output_tokens += raw.output_tokens;
        entry.input_tokens += raw.input_tokens;
        entry.cache_read_tokens += raw.cache_read_tokens;
        entry.cache_creation_tokens += raw.cache_creation_tokens;
        entry.cost_usd_micros += raw.cost_usd_micros;
    }
    for ((hour_epoch, site, app, _), model_bucket) in models {
        if let Some(hour_bucket) = merged.get_mut(&(hour_epoch, site.clone(), app.clone())) {
            hour_bucket.models.push(model_bucket);
        }
    }
    merged.into_values().collect()
}

/// P4b：跳闸事件按 (hour, provider, app) 计数。返回键与 RawBucket 的
/// provider 维度同构，复用同一张 hosts 映射。
fn query_breaker_trip_counts(
    db: &Database,
    after_epoch: i64,
    before_epoch: i64,
) -> Result<HashMap<(i64, String, String), i64>, AppError> {
    let conn = crate::database::lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT hour_epoch, provider_id, app_type, COUNT(*)              FROM crowd_breaker_events              WHERE hour_epoch > ?1 AND hour_epoch <= ?2              GROUP BY hour_epoch, provider_id, app_type",
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query(params![after_epoch, before_epoch])
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut counts = HashMap::new();
    while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        counts.insert((row.get(0)?, row.get(1)?, row.get(2)?), row.get(3)?);
    }
    Ok(counts)
}

/// P5：模型异常事件按 (hour, provider, app, model) 计数。返回键与
/// RawModelBucket 的 provider 维度同构，复用同一张 hosts 映射。
type ModelAnomalyCounts = HashMap<(i64, String, String, String), i64>;

/// 站点维度折叠的中间形状：(hour, site, app) → model → 异常次数。
type SiteAnomalyCounts = HashMap<(i64, String, String), HashMap<String, i64>>;

fn query_model_anomaly_counts(
    db: &Database,
    after_epoch: i64,
    before_epoch: i64,
) -> Result<ModelAnomalyCounts, AppError> {
    let conn = crate::database::lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT hour_epoch, provider_id, app_type, model, COUNT(*)              FROM crowd_model_anomaly_events              WHERE hour_epoch > ?1 AND hour_epoch <= ?2              GROUP BY hour_epoch, provider_id, app_type, model",
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query(params![after_epoch, before_epoch])
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut counts = HashMap::new();
    while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        counts.insert(
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?),
            row.get(4)?,
        );
    }
    Ok(counts)
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
    let model_raws = query_raw_model_buckets(db, after_epoch, before_epoch)?;
    let refs: HashSet<(String, String)> = raws
        .iter()
        .map(|raw| (raw.provider_id.clone(), raw.app_type.clone()))
        .collect();
    let hosts = resolve_relay_hosts(db, &refs)?;
    let mut merged = merge_by_site(raws, model_raws, &hosts);
    // P4b：跳闸计数并入。事件键是 provider 维度，先折成站点维度再对桶 ——
    // 与桶共用同一张 hosts 映射（未登记 provider 的事件自然丢弃，无主计数不出门）。
    let trips = query_breaker_trip_counts(db, after_epoch, before_epoch)?;
    let mut trips_by_site: HashMap<(i64, String, String), i64> = HashMap::new();
    for ((hour, provider, app), count) in &trips {
        if let Some(site) = hosts.get(&(provider.clone(), app.clone())) {
            *trips_by_site
                .entry((*hour, site.clone(), app.clone()))
                .or_insert(0) += count;
        }
    }
    for bucket in &mut merged {
        bucket.breaker_trips = trips_by_site
            .get(&(bucket.hour_epoch, bucket.site.clone(), bucket.app.clone()))
            .copied()
            .unwrap_or(0);
    }
    // P5：模型异常计数并入模型子桶。事件键是 (hour, provider, app, model)，
    // 先折成站点维度（同一张 hosts 映射，未登记 provider 丢弃），再对进已
    // 合并的模型行 —— 异常响应必然有对应的请求日志（tap 只挂在转发路径），
    // 模型行理论上恒命中；万一失配（日志被裁等）按「无主计数不出门」丢弃。
    let anomalies = query_model_anomaly_counts(db, after_epoch, before_epoch)?;
    let mut anomalies_by_site: SiteAnomalyCounts = HashMap::new();
    for ((hour, provider, app, model), count) in &anomalies {
        if let Some(site) = hosts.get(&(provider.clone(), app.clone())) {
            *anomalies_by_site
                .entry((*hour, site.clone(), app.clone()))
                .or_default()
                .entry(model.clone())
                .or_insert(0) += count;
        }
    }
    for bucket in &mut merged {
        if let Some(per_model) =
            anomalies_by_site.get(&(bucket.hour_epoch, bucket.site.clone(), bucket.app.clone()))
        {
            for model_row in &mut bucket.models {
                model_row.anomalies = per_model
                    .get(&model_row.model)
                    .copied()
                    .unwrap_or(model_row.anomalies);
            }
        }
    }
    Ok(merged)
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
    fn resolve_matches_providers_by_registrable_domain() {
        // 2026-09-05 的回归闸：站点的 API 域挂在 `api.` 子域（api_base_url 列）、
        // provider 的 base_url 指向它 —— 身份按注册域匹配，必须命中同一站。
        // 旧的「site_origin 全 host 匹配」在这里整桶丢弃（实测页空数据的根因）。
        let db = setup_db();
        {
            let conn = db.conn.lock().unwrap();
            crate::relay::creds::save_site(
                &conn,
                "https://panel.example",
                "中性示例站",
                "https://api.panel.example",
            )
            .unwrap();
        }
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES (?1, 'codex', ?2, ?3, '{}')",
                params![
                    "acct-a",
                    "示例档",
                    serde_json::json!({
                        "auth": {"OPENAI_API_KEY": "sk-test-not-a-real-key"},
                        "base_url": "https://api.panel.example/v1"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        seed_log(
            &db,
            "a",
            "acct-a",
            "codex",
            200,
            Some(250),
            "0.5",
            11 * 3600 + 100,
            "proxy",
        );

        let buckets = build_hour_buckets(&db, 0, 12 * 3600).unwrap();
        assert_eq!(
            buckets.len(),
            1,
            "api 子域上的 provider 必须归属到站点的注册域"
        );
        assert_eq!(buckets[0].site, "panel.example");
    }

    #[test]
    fn model_anomaly_events_fold_into_model_buckets() {
        let db = setup_db();
        {
            let conn = db.conn.lock().unwrap();
            crate::relay::creds::save_site(
                &conn,
                "https://panel.example",
                "中性示例站",
                "https://api.panel.example",
            )
            .unwrap();
        }
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('acct-a', 'codex', '示例档', ?1, '{}')",
                params![serde_json::json!({
                    "auth": {"OPENAI_API_KEY": "sk-test-not-a-real-key"},
                    "base_url": "https://api.panel.example/v1"
                })
                .to_string()],
            )
            .unwrap();
        seed_log(
            &db,
            "a",
            "acct-a",
            "codex",
            200,
            Some(250),
            "0.5",
            11 * 3600 + 100,
            "proxy",
        );
        // 两命事件 + 一条模型名对不上的 + 一条未登记 provider 的 —— 后两者
        // 都按「无主计数不出门」丢弃。
        for (provider, model) in [
            ("acct-a", "test-model"),
            ("acct-a", "test-model"),
            ("acct-a", "other-model"),
            ("ghost", "test-model"),
        ] {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO crowd_model_anomaly_events
                     (hour_epoch, provider_id, app_type, model, created_at)
                     VALUES (11 * 3600, ?1, 'codex', ?2, 0)",
                    params![provider, model],
                )
                .unwrap();
        }

        let buckets = build_hour_buckets(&db, 0, 12 * 3600).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].models.len(), 1, "模型行来自请求日志，恒为 1");
        assert_eq!(buckets[0].models[0].model, "test-model");
        assert_eq!(
            buckets[0].models[0].anomalies, 2,
            "只有命中同站点同模型的事件计数，其余丢弃"
        );
        assert_eq!(buckets[0].models[0].samples, 1);
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

    /// `errors` 只数站点侧失败：凭证/余额级（401/402/凭证级 403）与本机未出门的
    /// 503 不计，其余失败（429/5xx/网络 502/超时 504/站点侧 403）照数。
    /// 文案全部用 `error_mapper` 的真实输出播种 —— SQL 里的 403 前缀锚定与
    /// 本机 503 文案改任何一边，这条测试都会红（跨文件口径闸）。
    #[test]
    fn errors_count_site_side_failures_only() {
        use crate::proxy::ProxyError;
        let msg = crate::proxy::error_mapper::get_error_message;

        // (status, error_message, 是否应计入 errors)
        let cases: Vec<(i64, Option<String>, bool)> = vec![
            (200, None, false),
            (
                429,
                Some(msg(&ProxyError::UpstreamError {
                    status: 429,
                    body: Some("rate limited".to_string()),
                })),
                true,
            ),
            (
                500,
                Some(msg(&ProxyError::UpstreamError {
                    status: 500,
                    body: Some("internal".to_string()),
                })),
                true,
            ),
            (
                502,
                Some(msg(&ProxyError::ForwardFailed(
                    "connection refused".to_string(),
                ))),
                true,
            ),
            (
                504,
                Some(msg(&ProxyError::Timeout("first byte".to_string()))),
                true,
            ),
            // 凭证/余额级：上传者自己的账号问题，不抬站点公开错误率
            (
                401,
                Some(msg(&ProxyError::UpstreamError {
                    status: 401,
                    body: Some("无效令牌".to_string()),
                })),
                false,
            ),
            (
                402,
                Some(msg(&ProxyError::UpstreamError {
                    status: 402,
                    body: Some("余额不足，请充值".to_string()),
                })),
                false,
            ),
            // 站点侧 403（newapi 把站点故障包成 403，body 命中标记）：算站点的锅
            (
                403,
                Some(msg(&ProxyError::UpstreamError {
                    status: 403,
                    body: Some("上游线路余额不足，暂时无法完成请求".to_string()),
                })),
                true,
            ),
            (
                403,
                Some(msg(&ProxyError::UpstreamError {
                    status: 403,
                    body: Some("上游服务不可用".to_string()),
                })),
                true,
            ),
            // 凭证级 403（body 无站点侧标记）与无 body 的 403：不计
            (
                403,
                Some(msg(&ProxyError::UpstreamError {
                    status: 403,
                    body: Some("无效令牌".to_string()),
                })),
                false,
            ),
            (
                403,
                Some(msg(&ProxyError::UpstreamError {
                    status: 403,
                    body: None,
                })),
                false,
            ),
            // 本机未出门的 503：请求根本没到站点
            (503, Some(msg(&ProxyError::NoAvailableProvider)), false),
            (503, Some(msg(&ProxyError::AllProvidersCircuitOpen)), false),
            (503, Some(msg(&ProxyError::NoProvidersConfigured)), false),
            // 真上游 503：照数
            (
                503,
                Some(msg(&ProxyError::UpstreamError {
                    status: 503,
                    body: Some("service unavailable".to_string()),
                })),
                true,
            ),
        ];

        let db = setup_db();
        {
            let conn = db.conn.lock().unwrap();
            for (i, (status, message, _)) in cases.iter().enumerate() {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, status_code, first_token_ms,
                        total_cost_usd, latency_ms, created_at, data_source, error_message
                     ) VALUES (?1, 'p1', 'claude', 'm', ?2, NULL, '0', 0, ?3, 'proxy', ?4)",
                    params![format!("err-{i}"), status, 11 * 3600 + 100, message],
                )
                .unwrap();
            }
        }

        let raws = query_raw_buckets(&db, 0, 12 * 3600).unwrap();
        assert_eq!(raws.len(), 1);
        let expected: i64 = cases.iter().filter(|(_, _, counted)| *counted).count() as i64;
        assert_eq!(raws[0].samples, cases.len() as i64);
        assert_eq!(
            raws[0].errors, expected,
            "errors 只数站点侧失败（429/5xx/网络/超时/站点侧 403），\
             凭证余额级与本机未出门的不计"
        );
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

        let merged = merge_by_site(raws, Vec::new(), &hosts);
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
