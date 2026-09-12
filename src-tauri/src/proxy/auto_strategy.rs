//! Legacy setting keys plus shared model and pricing facts.
//! Policy ranking and affinity have been retired in favor of application priority.

use crate::database::Database;
use crate::provider::Provider;

/// settings 表里「某应用自动模式是否开启」的 key 前缀（`auto_mode_enabled_<app>`）。
pub const SETTING_ENABLED_PREFIX: &str = "auto_mode_enabled_";
/// settings 表里全局策略的 key（`cheapest` / `fastest`）。
pub const SETTING_STRATEGY: &str = "auto_mode_strategy";
/// settings 表里「某应用模型偏好」的 key 前缀（`auto_mode_model_<app>`）。
/// `None` = 不限模型（全部托管档位都进候选）。
pub const SETTING_MODEL_PREFIX: &str = "auto_mode_model_";
/// settings 表里「某应用省心选路模式」的 key 前缀（`easy_mode_mode_<app>`，
/// `auto` / `manual`）。
pub const SETTING_MODE_PREFIX: &str = "easy_mode_mode_";
/// settings 表里「某应用手动档位顺序」的 key 前缀（`easy_mode_manual_order_<app>`，
/// JSON 数组，存 provider id）。清单外的档位（新档位/被删后残留的 id）由下面的
/// 读取函数兜底：不认识的忽略、漏掉的按策略序追加 —— 手动序永远不能让档位丢失。
pub const SETTING_MANUAL_ORDER_PREFIX: &str = "easy_mode_manual_order_";

/// 读某应用的手动档位顺序。脏数据 / 不存在 → 空清单（等同「全按策略追加」）。
pub fn get_manual_order(db: &Database, app_type: &str) -> Vec<String> {
    db.get_setting(&format!("{SETTING_MANUAL_ORDER_PREFIX}{app_type}"))
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

/// 写某应用的手动档位顺序（完整清单，前端拖拽落定后整份提交）。
#[cfg(test)]
pub fn set_manual_order(
    db: &Database,
    app_type: &str,
    ordered_ids: &[String],
) -> Result<(), crate::error::AppError> {
    db.set_setting(
        &format!("{SETTING_MANUAL_ORDER_PREFIX}{app_type}"),
        &serde_json::to_string(ordered_ids)
            .map_err(|e| crate::error::AppError::Config(format!("序列化手动顺序失败: {e}")))?,
    )
}

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
#[cfg(test)]
pub fn is_auto_mode_enabled(db: &Database, app_type: &str) -> bool {
    db.get_setting(&format!("{SETTING_ENABLED_PREFIX}{app_type}"))
        .ok()
        .flatten()
        .is_some_and(|value| value == "true")
}

/// Request retries use only application fallback permission. Legacy mode state
/// is migrated at owner startup and cannot override this setting.
pub fn failover_active(_db: &Database, _app_type: &str, auto_failover_enabled: bool) -> bool {
    auto_failover_enabled
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
#[cfg(test)]
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

/// 档位的可用模型由模型目录领域负责；编辑映射不决定中转站的远端库存。
pub fn tier_models(tier: &Provider) -> Vec<String> {
    crate::relay::model_catalog::available_models(tier)
}

/// 自动模式的可选模型清单：该应用全部托管档位模型目录的**并集**（去重）。
///
/// 目录顺序沿用档位顺序 → 目录内顺序，跨档位重复只保留首次出现；返回空 =
/// 该应用没有模型目录（非 Codex 系），UI 放「不限模型」占位。托盘菜单与
/// 设置页共用这一份 —— 别在两边各拼一遍。
pub fn auto_mode_models(providers: &indexmap::IndexMap<String, Provider>) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for provider in providers.values() {
        for model in tier_models(provider) {
            if !models.contains(&model) {
                models.push(model);
            }
        }
    }
    models
}

/// 看板命令展示用的单价入口：与排序同一份实现（唯源），别在看板侧再算一遍。
pub(crate) fn effective_unit_price(
    db: &Database,
    tier: &Provider,
    model_pref: Option<&str>,
) -> Option<f64> {
    let conn = db
        .conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tier_unit_price(&conn, tier, model_pref)
}

/// 档位「有效模型」的合并单价：每百万 token 输入+输出之和（美元）。
///
/// 有效模型 = 模型偏好（调用方已按偏好过滤，候选目录都含它），没有偏好时用
/// 档位当前选中模型（`settings_config.config` 里的 `model`）。查不到价返回
/// `None` —— 排序侧当「未知」保守处理，绝不猜 0。
fn tier_unit_price(
    conn: &rusqlite::Connection,
    tier: &Provider,
    model_pref: Option<&str>,
) -> Option<f64> {
    let model = model_pref
        .map(str::to_string)
        .or_else(|| crate::relay::provision::extract_model(&tier.settings_config))?;
    let (input, output, _cache_read, _cache_creation) =
        crate::services::usage_stats::find_model_pricing_row(conn, &model).ok()??;
    let input: f64 = input.parse().ok()?;
    let output: f64 = output.parse().ok()?;
    Some(input + output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_mode_cannot_override_application_permission() {
        let db = Database::memory().unwrap();
        set_enabled(&db, "claude", true).unwrap();
        assert!(!failover_active(&db, "claude", false));
        assert!(failover_active(&db, "claude", true));
    }

    #[test]
    fn model_preference_remains_per_application_and_can_be_cleared() {
        let db = Database::memory().unwrap();
        set_model_pref(&db, "claude", Some("selected-model")).unwrap();
        assert_eq!(
            get_model_pref(&db, "claude").as_deref(),
            Some("selected-model")
        );
        assert_eq!(get_model_pref(&db, "codex"), None);
        set_model_pref(&db, "claude", None).unwrap();
        assert_eq!(get_model_pref(&db, "claude"), None);
    }
}
