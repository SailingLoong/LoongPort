//! 省心模式排序的健康信号（LoongPort）。
//!
//! 排序的两个体验维度——首字（TTFT）与稳定（站点侧错误率）——各自走同一条
//! 数据阶梯：**本地近窗实测优先，本地无样本回落众测站点快照，两者皆无按
//! 「无数据」参与排序**。无数据不奖不罚：与最差实测档同桶，桶内由下一级键
//! 决胜——冷启动（全部无数据）时排序退化为纯价格序，与上一代行为等价，
//! 数据长出来后好档位自然上浮。
//!
//! 错误口径唯源在 [`crate::crowd::bucket`] 的「站点侧失败」定义（剔除
//! 401/402、凭证级 403、本机未出门的 503——那些是账号问题，不是站点健康），
//! 众测上传与选路排序用同一把尺子，别在选路侧再发明一份。
//!
//! 读路径纪律：本模块只**读**既有产出——本地 usage 聚合与众测快照的本地
//! 缓存——绝不触发网络刷新。排序跑在请求路径上，快照刷新的 owner 是维护
//! 任务（`services::maintenance`）与广场命令，读路径不驱动数据层行为。

use std::collections::HashMap;

use crate::crowd::bucket::{PROXY_OBSERVED_EXPR, SITE_SIDE_ERROR_EXPR};
use crate::crowd::snapshot::{self, Snapshot, WindowStats};
use crate::database::Database;
use crate::error::AppError; // lock_conn! 宏展开引用，删除会被编译器请回来
use crate::provider::Provider;

/// 健康统计窗口：只看最近 7 天（更早的对「现在谁好」没有代表性，且窗口
/// 必须 ≤ 明细保留天数，prune 掉的数据不参与）。首字与错误率共用同一窗口。
pub(crate) const HEALTH_WINDOW_SECS: i64 = 7 * 86400;

/// 本地错误率的最低样本数：低于它的本地错误观测不可信（两次里错一次不是
/// 50% 错误率），该档位的错误率回落众测、再无则按无数据处理。
const LOCAL_ERR_MIN_SAMPLES: i64 = 8;

/// 众测快照参与排序的年龄上限：排序消费的是 w24 窗口，快照太老说明它描述
/// 的是过老的世界（共建关闭后残留的缓存也在这里自然失效）。刷新的 owner
/// 在维护任务与广场命令，选路只读——等不到新快照就安静地退回本地实测。
const SNAPSHOT_MAX_AGE_SECS: i64 = 48 * 3600;

/// 单个档位的健康信号（阶梯解析后的原始值，分桶在消费侧）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TierHealth {
    /// 近窗首字耗时（毫秒；本地为窗口平均，众测为 P50）。
    pub ttft_ms: Option<f64>,
    /// 站点侧错误率（0..=1）。
    pub err_rate: Option<f64>,
}

pub(crate) type TierHealthIndex = HashMap<String, TierHealth>;

/// 采集并按阶梯解析各档位的健康信号。`snapshot` 由调用方注入（选路只读
/// 本地缓存，见模块文档），`None` = 无可用快照；本函数不做任何拉取。
///
/// 逐指标独立走阶梯：首字有本地样本但错误样本不足时，首字用本地、错误率
/// 回落众测——两个维度各自用「最可信的那份数据」，不因一个维度缺数据
/// 整档作废。
pub(crate) fn collect(
    db: &Database,
    app_type: &str,
    tiers: &[Provider],
    now: i64,
    snapshot: Option<&Snapshot>,
) -> TierHealthIndex {
    let mut index = TierHealthIndex::new();
    if tiers.is_empty() {
        return index;
    }
    let local_ttft = db
        .get_provider_avg_first_token_ms(app_type, now - HEALTH_WINDOW_SECS)
        .unwrap_or_default();
    let local_err =
        query_site_side_error_counts(db, app_type, now - HEALTH_WINDOW_SECS).unwrap_or_default();
    let app_enum = app_type.parse::<crate::app_config::AppType>().ok();

    for tier in tiers {
        let crowd = snapshot.and_then(|snap| crowd_window(tier, app_enum.as_ref(), snap));
        index.insert(
            tier.id.clone(),
            TierHealth {
                ttft_ms: local_ttft
                    .get(&tier.id)
                    .map(|avg| *avg as f64)
                    .or_else(|| crowd.and_then(|w| w.ttft_p50_ms)),
                err_rate: local_err
                    .get(&tier.id)
                    .and_then(|(errors, samples)| {
                        (*samples >= LOCAL_ERR_MIN_SAMPLES)
                            .then_some(*errors as f64 / *samples as f64)
                    })
                    .or_else(|| crowd.and_then(|w| w.err_rate)),
            },
        );
    }
    index
}

/// 选路用的众测快照：共建门禁（关闭 = 完全不碰众测数据）+ 只读本地缓存 +
/// 年龄闸。不触发刷新——那归维护任务与广场命令。
pub(crate) fn cached_snapshot_for_ranking(now: i64) -> Option<Snapshot> {
    if !crate::settings::get_settings().crowd_metrics_enabled {
        return None;
    }
    let snapshot = snapshot::read_cached()?;
    snapshot_usable(&snapshot, now).then_some(snapshot)
}

/// 快照年龄闸（纯函数，测试钉边界）。
fn snapshot_usable(snapshot: &Snapshot, now: i64) -> bool {
    now - snapshot.generated_at <= SNAPSHOT_MAX_AGE_SECS
}

/// 档位的众测观测：provider 指纹归到注册域后查快照站点——与上传的站点归属
/// （`crowd::bucket::resolve_relay_hosts`）同一套解析，站-档两边不会各说各话。
/// 窗口取 w24，与广场排行同一份：不允许「广场说它快、选路说它慢」。
fn crowd_window<'a>(
    tier: &Provider,
    app_type: Option<&crate::app_config::AppType>,
    snapshot: &'a Snapshot,
) -> Option<&'a WindowStats> {
    let app_type = app_type?;
    let (origin, _api_key) = crate::relay::provider_fingerprint::for_provider(tier, app_type)?;
    let domain = crate::relay::identity::site_domain(&origin);
    snapshot
        .sites
        .get(&domain)
        .and_then(|site| site.w24.as_ref())
}

/// 首字分桶：粗桶——同桶内不认为有体验差别，交给下一级键决胜，让价格在
/// 「体验同级」的档位之间真正说话（桶切细了就退化成首字单要素排序）。
/// 无数据与最差档同桶（无数据不奖不罚）。
pub(crate) fn ttft_bucket(health: Option<&TierHealth>) -> u8 {
    match health.and_then(|h| h.ttft_ms) {
        Some(ms) if ms < 800.0 => 0,
        Some(ms) if ms < 2000.0 => 1,
        _ => 2,
    }
}

/// 站点侧错误率分桶：健康 / 劣化 / 糟糕。无数据与最差档同桶。
pub(crate) fn err_rate_bucket(health: Option<&TierHealth>) -> u8 {
    match health.and_then(|h| h.err_rate) {
        Some(rate) if rate < 0.02 => 0,
        Some(rate) if rate < 0.10 => 1,
        _ => 2,
    }
}

/// 近窗各档位的站点侧错误计数：`provider_id → (errors, err_samples)`。
/// 口径与上传桶完全同源（唯源见 [`crate::crowd::bucket`] 的两个表达式）：
/// 错误数只数站点的锅，分母只数本地代理亲历过的请求（session 回填行
/// status 恒 200，计入分母会把错误率拉向 0）。调用方查询失败按无数据处理
/// —— 统计缺位不能挡选路。
fn query_site_side_error_counts(
    db: &Database,
    app_type: &str,
    since: i64,
) -> Result<HashMap<String, (i64, i64)>, AppError> {
    let sql = format!(
        "SELECT l.provider_id, \
                SUM(CASE WHEN {site_side_error} THEN 1 ELSE 0 END), \
                SUM(CASE WHEN {proxy_observed} THEN 1 ELSE 0 END) \
         FROM proxy_request_logs l \
         WHERE l.app_type = ?1 AND l.created_at >= ?2 \
         GROUP BY l.provider_id",
        site_side_error = SITE_SIDE_ERROR_EXPR,
        proxy_observed = PROXY_OBSERVED_EXPR,
    );
    let conn = crate::database::lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![app_type, since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(id, errors, samples)| (id, (errors, samples)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crowd::snapshot::{SiteStats, WindowStats};
    use crate::provider::Provider;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// 桶边界是排序的「体验同级」判据，钉住边界值与「无数据=最差档」。
    #[test]
    fn buckets_pin_boundaries_and_none_is_worst() {
        let h = |ttft: Option<f64>, err: Option<f64>| TierHealth {
            ttft_ms: ttft,
            err_rate: err,
        };
        let no_data = TierHealth {
            ttft_ms: None,
            err_rate: None,
        };

        assert_eq!(ttft_bucket(Some(&h(Some(799.0), None))), 0);
        assert_eq!(ttft_bucket(Some(&h(Some(800.0), None))), 1);
        assert_eq!(ttft_bucket(Some(&h(Some(1999.9), None))), 1);
        assert_eq!(ttft_bucket(Some(&h(Some(2000.0), None))), 2);
        assert_eq!(ttft_bucket(Some(&no_data)), 2, "无数据与最差档同桶");
        assert_eq!(ttft_bucket(None), 2, "缺条目同样按无数据处理");

        assert_eq!(err_rate_bucket(Some(&h(None, Some(0.019)))), 0);
        assert_eq!(err_rate_bucket(Some(&h(None, Some(0.02)))), 1);
        assert_eq!(err_rate_bucket(Some(&h(None, Some(0.099)))), 1);
        assert_eq!(err_rate_bucket(Some(&h(None, Some(0.10)))), 2);
        assert_eq!(err_rate_bucket(Some(&no_data)), 2, "无数据与最差档同桶");
        assert_eq!(err_rate_bucket(None), 2);
    }

    #[test]
    fn snapshot_age_gate_bounds() {
        let snap = |generated_at: i64| Snapshot {
            version: 1,
            generated_at,
            sites: BTreeMap::new(),
            ttft_bin_edges: vec![],
        };
        let t = now();
        assert!(
            snapshot_usable(&snap(t - SNAPSHOT_MAX_AGE_SECS), t),
            "恰在年龄上限内可用"
        );
        assert!(
            !snapshot_usable(&snap(t - SNAPSHOT_MAX_AGE_SECS - 1), t),
            "超龄即弃用（退回本地实测）"
        );
    }

    fn seed_log(
        db: &Database,
        id: &str,
        provider: &str,
        status: i64,
        first_token_ms: Option<i64>,
        at: i64,
    ) {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, status_code, first_token_ms,
                    latency_ms, created_at, data_source
                 ) VALUES (?1, ?2, 'codex', 'm', ?3, ?4, 10, ?5, 'proxy')",
                rusqlite::params![id, provider, status, first_token_ms, at],
            )
            .unwrap();
    }

    fn tier(id: &str, base_url: Option<&str>) -> Provider {
        let settings = match base_url {
            Some(url) => json!({ "auth": { "OPENAI_API_KEY": "sk-test" }, "base_url": url }),
            None => json!({}),
        };
        Provider::with_id(id.to_string(), id.to_string(), settings, None)
    }

    fn snapshot_for(domain: &str, p50_ms: f64, err_rate: f64) -> Snapshot {
        Snapshot {
            version: 1,
            generated_at: now(),
            sites: BTreeMap::from([(
                domain.to_string(),
                SiteStats {
                    w24: Some(WindowStats {
                        samples: 500,
                        sources: 4,
                        ttft_p50_ms: Some(p50_ms),
                        ttft_p95_ms: None,
                        err_rate: Some(err_rate),
                        cache_hit_rate: None,
                        cost_usd_per_m_tok: None,
                        ttft_bins: vec![],
                    }),
                    w7: None,
                    hours: vec![],
                },
            )]),
            ttft_bin_edges: vec![],
        }
    }

    /// 阶梯第一优先级：本地样本足够时用本地，哪怕众测说得更漂亮。
    #[test]
    fn local_samples_win_over_crowd() {
        let db = Database::memory().unwrap();
        let t = now();
        let p = tier("p1", Some("https://api.a.example/v1"));
        // 本地 9 样本 3 错 = 33%（劣化桶），众测同站说 0%（健康桶）——必须信本地。
        for i in 0..6 {
            seed_log(&db, &format!("ok-{i}"), "p1", 200, Some(300), t - 60);
        }
        for i in 0..3 {
            seed_log(&db, &format!("err-{i}"), "p1", 500, None, t - 60);
        }
        let snap = snapshot_for("a.example", 150.0, 0.0);

        let index = collect(&db, "codex", std::slice::from_ref(&p), t, Some(&snap));
        let health = index.get("p1").unwrap();
        assert_eq!(health.err_rate, Some(3.0 / 9.0), "本地错误率优先于众测");
        assert_eq!(health.ttft_ms, Some(300.0), "本地首字平均优先于众测 P50");
    }

    /// 阶梯回落：本地样本不足（低于门槛）→ 该指标退回众测；本地完全没有 →
    /// 众测兜底；两边都没有 → 无数据。
    #[test]
    fn thin_or_missing_local_samples_fall_back_to_crowd() {
        let db = Database::memory().unwrap();
        let t = now();
        let thin = tier("thin", Some("https://api.a.example/v1"));
        let cold = tier("cold", Some("https://api.b.example/v1"));
        // thin：只有 2 条本地错误样本（低于门槛），不采信本地错误率
        seed_log(&db, "t-err-1", "thin", 500, None, t - 60);
        seed_log(&db, "t-err-2", "thin", 500, None, t - 60);
        let snap = Snapshot {
            version: 1,
            generated_at: t,
            sites: BTreeMap::from([
                (
                    "a.example".to_string(),
                    SiteStats {
                        w24: Some(WindowStats {
                            samples: 100,
                            sources: 3,
                            ttft_p50_ms: Some(900.0),
                            ttft_p95_ms: None,
                            err_rate: Some(0.05),
                            cache_hit_rate: None,
                            cost_usd_per_m_tok: None,
                            ttft_bins: vec![],
                        }),
                        w7: None,
                        hours: vec![],
                    },
                ),
                (
                    "b.example".to_string(),
                    SiteStats {
                        w24: Some(WindowStats {
                            samples: 80,
                            sources: 3,
                            ttft_p50_ms: Some(400.0),
                            ttft_p95_ms: None,
                            err_rate: Some(0.01),
                            cache_hit_rate: None,
                            cost_usd_per_m_tok: None,
                            ttft_bins: vec![],
                        }),
                        w7: None,
                        hours: vec![],
                    },
                ),
            ]),
            ttft_bin_edges: vec![],
        };

        let index = collect(&db, "codex", &[thin, cold], t, Some(&snap));
        assert_eq!(
            index.get("thin").unwrap().err_rate,
            Some(0.05),
            "错误样本不足（2 < 门槛）回落众测"
        );
        assert_eq!(
            index.get("thin").unwrap().ttft_ms,
            Some(900.0),
            "首字无本地样本（错误行无计时）回落众测 P50——两个指标各自走阶梯"
        );
        let cold_health = index.get("cold").unwrap();
        assert_eq!(cold_health.ttft_ms, Some(400.0));
        assert_eq!(cold_health.err_rate, Some(0.01));
    }

    /// 无指纹（配置不成形状）或快照没收录的站：众测维度按无数据处理，
    /// 本地维度照常。
    #[test]
    fn unknown_or_fingerprintless_tiers_get_no_crowd_data() {
        let db = Database::memory().unwrap();
        let t = now();
        let no_fp = tier("no-fp", None);
        let unknown_site = tier("unknown-site", Some("https://api.nobody.example/v1"));
        let snap = snapshot_for("a.example", 150.0, 0.0);

        let index = collect(&db, "codex", &[no_fp, unknown_site], t, Some(&snap));
        assert_eq!(
            index.get("no-fp").unwrap(),
            &TierHealth {
                ttft_ms: None,
                err_rate: None
            }
        );
        assert_eq!(
            index.get("unknown-site").unwrap(),
            &TierHealth {
                ttft_ms: None,
                err_rate: None
            }
        );
    }

    /// 口径闸（选路消费端）：账号级失败（401/402）不是站点健康问题——
    /// 10 条代理样本里 1 条 401，错误率必须是 0%（健康桶），而不是 10%。
    #[test]
    fn account_level_failures_do_not_poison_error_rate() {
        let db = Database::memory().unwrap();
        let t = now();
        for i in 0..9 {
            seed_log(&db, &format!("ok-{i}"), "p1", 200, Some(300), t - 60);
        }
        seed_log(&db, "auth-err", "p1", 401, None, t - 60);
        let p = tier("p1", Some("https://api.a.example/v1"));

        let index = collect(&db, "codex", &[p], t, None);
        assert_eq!(
            index.get("p1").unwrap().err_rate,
            Some(0.0),
            "401 是账号问题，不抬站点的排序错误率（口径与上传同源）"
        );
    }
}
