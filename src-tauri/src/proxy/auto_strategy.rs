//! 自动模式策略排序（LoongPort）。
//!
//! 产品语义：用户只选 app（和模型，M3），系统从可用托管档位里按策略挑最合适的：
//! - **Cheapest（默认）**：按 `providers.tier_rate_multiplier` 升序；
//! - **Fastest**：按最近窗口的平均首字耗时（TTFT）升序。
//!
//! ## 会话亲和（硬需求）
//!
//! 同一会话中途切换供应商会丢失提示词缓存，未命中缓存的请求按全价计费 ——
//! 所以**当前在用档位只要近期还有流量，就保持置顶**，策略重排只影响它身后的
//! 候选顺序（当前档位故障熔断后自然落到重排结果上）。闲置超过亲和窗口，
//! 重排才真正接管（下一批请求由策略第一名服务，成功后热切换）。
//!
//! ## 为什么是独立模块
//!
//! `provider_router` / `circuit_breaker` 来自上游，选路骨架保持最小改动；
//! 自动模式的排序判据、窗口、亲和规则全是 LoongPort 语义，收在这里。

use crate::database::Database;
use crate::provider::Provider;
use std::cmp::Ordering;
use std::str::FromStr;

/// settings 表里「某应用自动模式是否开启」的 key 前缀（`auto_mode_enabled_<app>`）。
pub const SETTING_ENABLED_PREFIX: &str = "auto_mode_enabled_";
/// settings 表里全局策略的 key（`cheapest` / `fastest`）。
pub const SETTING_STRATEGY: &str = "auto_mode_strategy";
/// settings 表里「某应用模型偏好」的 key 前缀（`auto_mode_model_<app>`）。
/// `None` = 不限模型（全部托管档位都进候选）。
pub const SETTING_MODEL_PREFIX: &str = "auto_mode_model_";

/// 会话亲和窗口：当前档位最近一次请求距今小于该值即视为「会话进行中」。
/// 30 分钟 ≈ 一次长编码会话的自然间隔，期间不因策略重排切走。
/// 公开给 `provider_router`（非托管当前供应商的置顶判断用同一窗口，别两处各写一份）。
pub const AFFINITY_WINDOW_SECS: i64 = 30 * 60;

/// TTFT 统计窗口：只看最近 7 天的首字耗时（更早的对「现在谁快」没有代表性，
/// 且窗口必须 ≤ 明细保留天数，prune 掉的数据不参与）。
const TTFT_WINDOW_SECS: i64 = 7 * 86400;

/// 自动模式策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStrategy {
    /// 价格最低：按档位倍率升序（默认）。
    Cheapest,
    /// 响应最快：按平均首字耗时升序。
    Fastest,
}

impl AutoStrategy {
    /// 从 settings 值解析；不认识的值落回默认（cheapest），别让脏数据炸掉选路。
    pub fn from_setting_value(value: &str) -> Self {
        match value {
            "fastest" => Self::Fastest,
            _ => Self::Cheapest,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cheapest => "cheapest",
            Self::Fastest => "fastest",
        }
    }
}

/// 某应用的自动模式是否开启（settings 表，缺省 false）。
pub fn is_auto_mode_enabled(db: &Database, app_type: &str) -> bool {
    db.get_setting(&format!("{SETTING_ENABLED_PREFIX}{app_type}"))
        .ok()
        .flatten()
        .is_some_and(|value| value == "true")
}

/// 读全局策略（缺省 cheapest）。
pub fn get_strategy(db: &Database) -> AutoStrategy {
    db.get_setting(SETTING_STRATEGY)
        .ok()
        .flatten()
        .map(|value| AutoStrategy::from_setting_value(&value))
        .unwrap_or(AutoStrategy::Cheapest)
}

/// 写入某应用的自动模式开关。
pub fn set_enabled(
    db: &Database,
    app_type: &str,
    enabled: bool,
) -> Result<(), crate::error::AppError> {
    db.set_setting(
        &format!("{SETTING_ENABLED_PREFIX}{app_type}"),
        if enabled { "true" } else { "false" },
    )
}

/// 写入全局策略。
pub fn set_strategy(db: &Database, strategy: AutoStrategy) -> Result<(), crate::error::AppError> {
    db.set_setting(SETTING_STRATEGY, strategy.as_str())
}

/// 读某应用的模型偏好（`None` = 不限模型）。
pub fn get_model_pref(db: &Database, app_type: &str) -> Option<String> {
    db.get_setting(&format!("{SETTING_MODEL_PREFIX}{app_type}"))
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
}

/// 写某应用的模型偏好（`None` 清除，回到不限）。
pub fn set_model_pref(
    db: &Database,
    app_type: &str,
    model: Option<&str>,
) -> Result<(), crate::error::AppError> {
    db.set_setting(
        &format!("{SETTING_MODEL_PREFIX}{app_type}"),
        model.unwrap_or(""),
    )
}

/// 档位的模型目录（settings_config.modelCatalog）。目前只有 Codex 系托管档位
/// 在 provision 时写目录；返回空 = 该档位没有目录（不参与模型过滤）。
pub fn tier_models(tier: &Provider) -> Vec<String> {
    crate::commands::models_from_settings(&tier.settings_config)
}

/// 按模型偏好过滤候选：只保留目录里含偏好模型的档位。
///
/// 两类回退都不拦人：偏好的模型已从所有目录下架（过滤后为空）→ 回退全量，
/// 选路不能因为一个过期偏好就没档位可用；该应用根本没有目录（非 Codex 系）
/// → 过滤无意义，直接全量。无目录档位在有偏好时**不**保留 —— 目录都没有，
/// 无法证明它能服务这个模型。
fn filter_by_model_pref(tiers: Vec<Provider>, model_pref: Option<&str>) -> Vec<Provider> {
    let Some(model) = model_pref else {
        return tiers;
    };
    let has_catalogs = tiers.iter().any(|t| !tier_models(t).is_empty());
    if !has_catalogs {
        return tiers;
    }
    let filtered: Vec<Provider> = tiers
        .iter()
        .filter(|t| tier_models(t).iter().any(|m| m == model))
        .cloned()
        .collect();
    if filtered.is_empty() {
        log::warn!("[AutoMode] 模型偏好 {model} 不在任何档位目录中，回退全量候选");
        return tiers;
    }
    filtered
}

/// 当前供应商 id：本地 settings 优先（校验存在性），fallback 到数据库 is_current。
/// `provider_router` 的常规选路与自动模式候选共用这一份解析。
pub fn effective_current_provider_id(db: &Database, app_type: &str) -> Option<String> {
    crate::app_config::AppType::from_str(app_type)
        .ok()
        .and_then(|app_enum| {
            crate::settings::get_effective_current_provider(db, &app_enum)
                .ok()
                .flatten()
        })
        .or_else(|| db.get_current_provider(app_type).ok().flatten())
}

/// 自动模式候选（选路与「开启即切最优/选模型」命令共用，唯源）：
/// 该应用全部托管档位，按模型偏好过滤后按策略排序；当前在用档位（含非托管）
/// 会话活跃时置顶。没有任何托管档位时返回 `None`（调用方回退常规选路 / 拒绝开启）。
///
/// 当前在用的是**非托管**供应商（用户自选的官网直连等）且会话活跃时，同样置顶 ——
/// 亲和规则的判据是「切换丢缓存」，与在用的是不是托管档位无关；用户的手动选择
/// 在他闲置或该供应商熔断之前不被系统挤走。
///
/// `honor_affinity`：托盘点选模型是**显式**选择，绕过亲和立即切到目标 ——
/// 亲和保护的是「系统重排别打断会话」，不是替用户拒绝他刚点的选择。
pub fn rank_managed_tier_candidates(
    db: &Database,
    app_type: &str,
    honor_affinity: bool,
) -> Result<Option<Vec<Provider>>, crate::error::AppError> {
    let tiers: Vec<Provider> = db
        .get_all_providers(app_type)?
        .values()
        .filter(|p| crate::relay::is_managed(&p.id))
        .cloned()
        .collect();

    if tiers.is_empty() {
        return Ok(None);
    }

    let tiers = filter_by_model_pref(tiers, get_model_pref(db, app_type).as_deref());

    let current_id = if honor_affinity {
        effective_current_provider_id(db, app_type)
    } else {
        None
    };
    let now = chrono::Utc::now().timestamp();
    let mut ranked = rank_tiers(
        db,
        app_type,
        &tiers,
        current_id.as_deref(),
        get_strategy(db),
        now,
    );

    if let Some(current_id) = current_id.as_deref() {
        let already_first = ranked.first().is_some_and(|p| p.id == current_id);
        if !already_first {
            let session_active = db
                .get_provider_last_activity(app_type)?
                .get(current_id)
                .is_some_and(|last| now - *last < AFFINITY_WINDOW_SECS);
            if session_active {
                if let Some(current) = db.get_provider_by_id(current_id, app_type)? {
                    ranked.insert(0, current);
                }
            }
        }
    }

    Ok(Some(ranked))
}

/// 对托管档位按策略排序；当前在用档位在亲和窗口内保持置顶。
///
/// `now` 由调用方注入（unix 秒），测试里可以拨时钟。
/// 排序键带上档位 id 做最终 tie-breaker，保证结果确定（同名倍率/同无样本时
/// 不会因 HashMap 遍历序而抖动）。
pub fn rank_tiers(
    db: &Database,
    app_type: &str,
    tiers: &[Provider],
    current_id: Option<&str>,
    strategy: AutoStrategy,
    now: i64,
) -> Vec<Provider> {
    let multipliers = db.get_tier_rate_multipliers(app_type).unwrap_or_default();
    let ttft = db
        .get_provider_avg_first_token_ms(app_type, now - TTFT_WINDOW_SECS)
        .unwrap_or_default();
    let last_activity = db.get_provider_last_activity(app_type).unwrap_or_default();

    let cost_of =
        |p: &Provider| -> f64 { multipliers.get(&p.id).copied().unwrap_or(f64::INFINITY) };
    let ttft_of = |p: &Provider| -> u64 { ttft.get(&p.id).copied().unwrap_or(u64::MAX) };

    let mut ranked = tiers.to_vec();
    ranked.sort_by(|a, b| {
        let primary = match strategy {
            AutoStrategy::Cheapest => cost_of(a)
                .partial_cmp(&cost_of(b))
                .unwrap_or(Ordering::Equal),
            AutoStrategy::Fastest => ttft_of(a).cmp(&ttft_of(b)),
        };
        // 次级键互为 fallback： cheapest 时更快者先（同价选快的），
        // fastest 时更便宜者先（同速选便宜的），冷启动（无 TTFT 样本）也能有序。
        let secondary = match strategy {
            AutoStrategy::Cheapest => ttft_of(a).cmp(&ttft_of(b)),
            AutoStrategy::Fastest => cost_of(a)
                .partial_cmp(&cost_of(b))
                .unwrap_or(Ordering::Equal),
        };
        primary.then(secondary).then_with(|| a.id.cmp(&b.id))
    });

    // 会话亲和：当前档位近期活跃 → 置顶（见模块文档）。不活跃/无记录则不动。
    if let Some(current_id) = current_id {
        let session_active = last_activity
            .get(current_id)
            .is_some_and(|last| now - *last < AFFINITY_WINDOW_SECS);
        if session_active {
            if let Some(pos) = ranked.iter().position(|p| p.id == current_id) {
                let current = ranked.remove(pos);
                ranked.insert(0, current);
            }
        }
    }

    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::relay::provision;
    use serde_json::json;

    /// 生成一个真实形状的托管档位 id（钉住 is_managed 判据，别手写假的）。
    fn managed_id(site: &str, group: i64, salt: i64) -> String {
        let id = provision::provider_id_for(site, Some(group), salt);
        assert!(crate::relay::is_managed(&id));
        id
    }

    fn tier(id: &str, name: &str) -> Provider {
        Provider::with_id(id.to_string(), name.to_string(), json!({}), None)
    }

    fn seed_activity(db: &Database, app_type: &str, provider_id: &str, at: i64, ttft_ms: i64) {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, output_tokens, total_cost_usd,
                latency_ms, first_token_ms, status_code, created_at
            ) VALUES (?1, ?2, ?3, 'm', 1, 1, '0', 10, ?4, 200, ?5)",
            rusqlite::params![
                format!("act-{provider_id}-{at}-{ttft_ms}"),
                provider_id,
                app_type,
                ttft_ms,
                at
            ],
        )
        .unwrap();
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    #[test]
    fn cheapest_orders_by_multiplier_with_unknown_last() {
        let db = Database::memory().unwrap();
        let cheap = managed_id("https://a.example", 1, 1);
        let mid = managed_id("https://b.example", 1, 2);
        let unknown = managed_id("https://c.example", 1, 3);
        let tiers = vec![tier(&unknown, "U"), tier(&cheap, "C"), tier(&mid, "M")];

        // 倍率是 providers 表的列：行不存在时 UPDATE 静默落空，必须先存行
        for p in &tiers {
            db.save_provider("claude", p).unwrap();
        }
        db.set_tier_rate_multiplier("claude", &cheap, Some(0.5))
            .unwrap();
        db.set_tier_rate_multiplier("claude", &mid, Some(1.2))
            .unwrap();

        let ranked = rank_tiers(&db, "claude", &tiers, None, AutoStrategy::Cheapest, now());
        let ids: Vec<&str> = ranked.iter().map(|p| p.id.as_str()).collect();
        // 倍率未知（None）当最贵处理，排最后 —— 别让「不知道价格」变成「最便宜」
        assert_eq!(ids, vec![cheap.as_str(), mid.as_str(), unknown.as_str()]);
    }

    #[test]
    fn fastest_orders_by_ttft_with_sampleless_last() {
        let db = Database::memory().unwrap();
        let fast = managed_id("https://a.example", 1, 1);
        let slow = managed_id("https://b.example", 1, 2);
        let cold = managed_id("https://c.example", 1, 3);
        let tiers = vec![tier(&cold, "Cold"), tier(&slow, "S"), tier(&fast, "F")];

        let t = now();
        seed_activity(&db, "claude", &fast, t - 60, 120);
        seed_activity(&db, "claude", &slow, t - 60, 900);

        let ranked = rank_tiers(&db, "claude", &tiers, None, AutoStrategy::Fastest, t);
        let ids: Vec<&str> = ranked.iter().map(|p| p.id.as_str()).collect();
        // 有样本的按耗时升序；无 TTFT 样本（cold）排最后 —— 冷启动不抢跑
        assert_eq!(ids, vec![fast.as_str(), slow.as_str(), cold.as_str()]);
    }

    #[test]
    fn affinity_hoists_recently_active_current_tier() {
        let db = Database::memory().unwrap();
        let expensive_current = managed_id("https://a.example", 1, 1);
        let cheap = managed_id("https://b.example", 1, 2);
        let tiers = vec![tier(&expensive_current, "Cur"), tier(&cheap, "C")];

        for p in &tiers {
            db.save_provider("claude", p).unwrap();
        }
        db.set_tier_rate_multiplier("claude", &expensive_current, Some(2.0))
            .unwrap();
        db.set_tier_rate_multiplier("claude", &cheap, Some(0.5))
            .unwrap();

        let t = now();
        // 当前档位 10 分钟前还有流量 → 会话进行中，保持置顶
        seed_activity(&db, "claude", &expensive_current, t - 600, 500);

        let ranked = rank_tiers(
            &db,
            "claude",
            &tiers,
            Some(&expensive_current),
            AutoStrategy::Cheapest,
            t,
        );
        assert_eq!(ranked[0].id, expensive_current);

        // 闲置超过亲和窗口 → 重排接管，最便宜的回到第一
        let ranked_idle = rank_tiers(
            &db,
            "claude",
            &tiers,
            Some(&expensive_current),
            AutoStrategy::Cheapest,
            t + AFFINITY_WINDOW_SECS + 1,
        );
        assert_eq!(ranked_idle[0].id, cheap);
    }

    #[test]
    fn strategy_setting_roundtrip_and_default() {
        let db = Database::memory().unwrap();
        assert_eq!(get_strategy(&db), AutoStrategy::Cheapest);

        set_strategy(&db, AutoStrategy::Fastest).unwrap();
        assert_eq!(get_strategy(&db), AutoStrategy::Fastest);

        // 脏数据不炸选路：落回默认
        db.set_setting(SETTING_STRATEGY, "nonsense").unwrap();
        assert_eq!(get_strategy(&db), AutoStrategy::Cheapest);
    }

    #[test]
    fn enabled_flag_roundtrip() {
        let db = Database::memory().unwrap();
        assert!(!is_auto_mode_enabled(&db, "claude"));

        set_enabled(&db, "claude", true).unwrap();
        assert!(is_auto_mode_enabled(&db, "claude"));
        // 按 app 隔离
        assert!(!is_auto_mode_enabled(&db, "codex"));

        set_enabled(&db, "claude", false).unwrap();
        assert!(!is_auto_mode_enabled(&db, "claude"));
    }

    /// 模型偏好过滤：有偏好时只留「目录含该模型」的档位（无目录档位不保留 ——
    /// 无法证明它能服务这个模型）；偏好过期（不在任何目录）回退全量；
    /// 无偏好全量。经 rank_managed_tier_candidates（honor_affinity=false）走真实路径。
    #[test]
    fn model_pref_filters_candidates_with_stale_fallback() {
        let db = Database::memory().unwrap();
        let has_sol = managed_id("https://a.example", 1, 1);
        let has_nano = managed_id("https://b.example", 1, 2);
        let no_catalog = managed_id("https://c.example", 1, 3);

        let with_catalog =
            |model: &str| serde_json::json!({ "modelCatalog": { "models": [{ "model": model }] } });
        for (id, settings) in [
            (&has_sol, with_catalog("gpt-5.6-sol")),
            (&has_nano, with_catalog("gpt-5.6-nano")),
            (&no_catalog, serde_json::json!({})),
        ] {
            let mut p = tier(id, id);
            p.settings_config = settings;
            db.save_provider("codex", &p).unwrap();
        }

        // 偏好 sol → 只剩目录含 sol 的档位（无目录档位被排除）
        set_model_pref(&db, "codex", Some("gpt-5.6-sol")).unwrap();
        let ranked = rank_managed_tier_candidates(&db, "codex", false)
            .unwrap()
            .expect("候选不应为空");
        let ids: Vec<&str> = ranked.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec![has_sol.as_str()]);

        // 过期偏好（不在任何目录）→ 回退全量，选路不能没档位可用
        set_model_pref(&db, "codex", Some("gone-model")).unwrap();
        let ranked = rank_managed_tier_candidates(&db, "codex", false)
            .unwrap()
            .expect("过期偏好必须回退全量");
        assert_eq!(ranked.len(), 3);

        // 无偏好 → 全量
        set_model_pref(&db, "codex", None).unwrap();
        let ranked = rank_managed_tier_candidates(&db, "codex", false)
            .unwrap()
            .expect("无偏好全量");
        assert_eq!(ranked.len(), 3);

        // 读写往返
        assert_eq!(get_model_pref(&db, "codex"), None);
        set_model_pref(&db, "codex", Some("gpt-5.6-nano")).unwrap();
        assert_eq!(
            get_model_pref(&db, "codex").as_deref(),
            Some("gpt-5.6-nano")
        );
    }
}
