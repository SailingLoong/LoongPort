//! Persistent application priority. Views and the proxy read the same order;
//! legacy routing state is retired by startup/mutations, never by reads.
use super::{auto_strategy, provider_router::provider_supports_failover};
use crate::{
    app_config::AppType,
    database::{lock_conn, Database},
    error::AppError,
    provider::Provider,
};
use rusqlite::OptionalExtension;
use std::{collections::HashSet, str::FromStr};

fn priority_key(app: &str) -> String {
    format!("application_priority_{app}")
}

/// 用户屏蔽的档位名单（按 app 持久化，形状与优先级序同款 settings JSON 列表）。
fn blocked_key(app: &str) -> String {
    format!("application_blocked_{app}")
}

pub fn blocked_tier_ids(db: &Database, app: &str) -> HashSet<String> {
    db.get_setting(&blocked_key(app))
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .map(|ids| ids.into_iter().collect())
        .unwrap_or_default()
}

/// 屏蔽是用户显式意图：写入即生效（选路侧每次现读，无需失效通知）。
pub fn set_tier_blocked(
    db: &Database,
    app: &str,
    provider_id: &str,
    blocked: bool,
) -> Result<(), AppError> {
    if !ordered_providers(db, app)?
        .iter()
        .any(|p| p.id == provider_id)
    {
        return Err(AppError::Config("Unknown provider for this app".into()));
    }
    let mut ids = blocked_tier_ids(db, app);
    if blocked {
        ids.insert(provider_id.to_string());
    } else {
        ids.remove(provider_id);
    }
    let ordered: Vec<String> = ids.into_iter().collect();
    db.set_setting(
        &blocked_key(app),
        &serde_json::to_string(&ordered).map_err(|e| AppError::Config(e.to_string()))?,
    )
}

pub fn ordered_providers(db: &Database, app: &str) -> Result<Vec<Provider>, AppError> {
    AppType::from_str(app)?;
    let order = stored_order(db, app)?;
    let mut providers: Vec<_> = db.get_all_providers(app)?.into_values().collect();
    providers.sort_by(|a, b| {
        let rank = |id: &str| {
            order
                .iter()
                .position(|entry| entry == id)
                .unwrap_or(usize::MAX)
        };
        rank(&a.id)
            .cmp(&rank(&b.id))
            .then(
                a.sort_index
                    .unwrap_or(usize::MAX)
                    .cmp(&b.sort_index.unwrap_or(usize::MAX)),
            )
            .then(a.id.cmp(&b.id))
    });
    Ok(providers)
}

/// 存储链的原始 id 列表（可能含上游已删除的幽灵）；链未初始化时为 None。
fn stored_order(db: &Database, app: &str) -> Result<Vec<String>, AppError> {
    Ok(db
        .get_setting(&priority_key(app))?
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|e| AppError::Config(e.to_string()))?
        .unwrap_or_default())
}

/// 故障切换链的有效 id 全集（2026-09-16 用户定调：链 = 用户已应用的列表）。
///
/// - 链已初始化：严格按存储链返回，幽灵原样带出（调用方各自跳过）。
///   `set_order` 拒绝空列表，所以已初始化的链绝不退化成空表。
/// - 链未初始化：回落全量显示序——与启动 migrate 的全量播种等价的读时默认，
///   让「从没应用过」和「应用了全部」在读取侧无歧义地同形。
pub fn chain_ids(db: &Database, app: &str) -> Result<Vec<String>, AppError> {
    AppType::from_str(app)?;
    match db.get_setting(&priority_key(app))? {
        Some(raw) => serde_json::from_str(&raw).map_err(|e| AppError::Config(e.to_string())),
        None => Ok(ordered_providers(db, app)?
            .into_iter()
            .map(|p| p.id)
            .collect()),
    }
}

/// 链成员（按存储序，幽灵跳过）——选路与故障切换重试的全集与顺序唯源。
/// 链外档位不是后备：被用户应用出链的档位永不参与自动重试。
pub fn chain_providers(db: &Database, app: &str) -> Result<Vec<Provider>, AppError> {
    let ids = chain_ids(db, app)?;
    let providers = db.get_all_providers(app)?;
    Ok(ids
        .into_iter()
        .filter_map(|id| providers.get(&id).cloned())
        .collect())
}

/// 新档位自动进链垫底（上游新增默认排在最后生效）。
///
/// 只在链已初始化且 id 不在链里时追加；未初始化交给 migrate 全量播种，不抢跑。
/// 只由 [`crate::database::Database::save_provider`] 的插入分支调用——编辑更新
/// 不追加，否则被用户应用出链的档位一刷新就爬回链里。
pub fn note_provider_created(db: &Database, app: &str, id: &str) -> Result<(), AppError> {
    let key = priority_key(app);
    let Some(raw) = db.get_setting(&key)? else {
        return Ok(());
    };
    let mut order: Vec<String> =
        serde_json::from_str(&raw).map_err(|e| AppError::Config(e.to_string()))?;
    if order.iter().any(|entry| entry == id) {
        return Ok(());
    }
    order.push(id.to_string());
    db.set_setting(
        &key,
        &serde_json::to_string(&order).map_err(|e| AppError::Config(e.to_string()))?,
    )
}

/// Read local selection without the legacy getter's stale-setting cleanup.
pub fn current_provider_id(db: &Database, app: &str) -> Option<String> {
    let app_type = AppType::from_str(app).ok()?;
    crate::settings::get_current_provider(&app_type)
        .filter(|id| db.get_provider_by_id(id, app).ok().flatten().is_some())
        .or_else(|| db.get_current_provider(app).ok().flatten())
}

/// A static exclusion shared by route selection and its presentation.
/// 调用方在循环外预载屏蔽名单传入（选路与看板都逐档位调用，避免逐次查 settings）。
/// 屏蔽排在最前：用户显式意图压过能力/模型等推断性原因。
pub fn fallback_exclusion_with(
    db: &Database,
    app: &str,
    provider: &Provider,
    blocked: &HashSet<String>,
) -> Option<&'static str> {
    if blocked.contains(&provider.id) {
        return Some("blocked");
    }
    if !super::provider_router::provider_supports_proxy_routing(app, provider) {
        return Some("native_configuration");
    }
    if !provider_supports_failover(app, provider) {
        return Some("official_account");
    }
    if let Some(model) = effective_model(db, app) {
        if !auto_strategy::tier_models(provider).contains(&model) {
            return Some("model_incompatible");
        }
    }
    None
}

/// Preserve explicit legacy order once, then retire both old mode and queue.
pub fn migrate(db: &Database, app: &str) -> Result<(), AppError> {
    let key = priority_key(app);
    let mut ids: Vec<String> = ordered_providers(db, app)?
        .into_iter()
        .map(|p| p.id)
        .collect();
    let initialized = db.get_setting(&key)?.is_some();
    if !initialized {
        let legacy = auto_strategy::get_manual_order(db, app);
        let legacy = if legacy.is_empty() {
            db.get_failover_queue(app)?
                .into_iter()
                .map(|p| p.provider_id)
                .collect()
        } else {
            legacy
        };
        ids.sort_by_key(|id| legacy.iter().position(|p| p == id).unwrap_or(usize::MAX));
    }
    let order = serde_json::to_string(&ids).map_err(|e| AppError::Config(e.to_string()))?;
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    if !initialized {
        tx.execute("UPDATE proxy_config SET auto_failover_enabled = 1 WHERE app_type = ?1 AND EXISTS (SELECT 1 FROM settings WHERE key = ?2 AND value = 'true') AND NOT EXISTS (SELECT 1 FROM settings WHERE key = ?3)", rusqlite::params![app, format!("{}{}", auto_strategy::SETTING_ENABLED_PREFIX, app), key])?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO settings (key,value) VALUES (?1,?2)",
        rusqlite::params![key, order],
    )?;
    for prefix in [
        auto_strategy::SETTING_ENABLED_PREFIX,
        auto_strategy::SETTING_MODE_PREFIX,
        auto_strategy::SETTING_MANUAL_ORDER_PREFIX,
    ] {
        tx.execute(
            "DELETE FROM settings WHERE key = ?1",
            [format!("{prefix}{app}")],
        )?;
    }
    tx.execute(
        "UPDATE providers SET in_failover_queue = 0 WHERE app_type = ?1",
        [app],
    )?;
    tx.commit()?;
    Ok(())
}

/// 写入链 = 用户「应用此顺序」的载荷：当前可见且未屏蔽的档位按显示序，恰好这么多。
///
/// 不再垫底（2026-09-16 定调）：被筛出视图的档位不是后备，链外档位永不参与自动重试。
/// 新档位由 [`note_provider_created`] 在创建时自动垫底；上游删掉的档位以幽灵形式
/// 留在链里（选路跳过），用户下次应用即清理。空列表拒绝——故障切换链至少要有一个成员。
pub fn set_order(db: &Database, app: &str, ids: &[String]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Err(AppError::Config("Application chain cannot be empty".into()));
    }
    let providers = ordered_providers(db, app)?;
    let mut seen = HashSet::new();
    if ids
        .iter()
        .any(|id| !seen.insert(id.clone()) || !providers.iter().any(|p| p.id == *id))
    {
        return Err(AppError::Config(
            "Priority contains duplicate or unknown providers".into(),
        ));
    }
    migrate(db, app)?;
    db.set_setting(
        &priority_key(app),
        &serde_json::to_string(ids).map_err(|e| AppError::Config(e.to_string()))?,
    )
}

pub async fn set_failover(db: &Database, app: &str, enabled: bool) -> Result<(), AppError> {
    if !AppType::from_str(app)?.supports_local_proxy() {
        return Err(AppError::Config(
            "Application does not support local proxy routing".into(),
        ));
    }
    // Only the permission column belongs to this mutation. Never write back
    // a stale full proxy config over concurrent timeout or takeover changes.
    if enabled {
        let conn = lock_conn!(db.conn);
        let taken_over = conn
            .query_row(
                "SELECT enabled FROM proxy_config WHERE app_type = ?1",
                [app],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if !taken_over {
            return Err(AppError::Config(
                "Enable application proxy takeover first".into(),
            ));
        }
    }
    migrate(db, app)?;
    let conn = lock_conn!(db.conn);
    let changed = conn.execute("UPDATE proxy_config SET auto_failover_enabled = ?2 WHERE app_type = ?1 AND (?2 = 0 OR enabled = 1)", rusqlite::params![app, enabled])?;
    if enabled && changed == 0 {
        return Err(AppError::Config(
            "Enable application proxy takeover first".into(),
        ));
    }
    Ok(())
}

/// Pure read: missing proxy rows mean fallback is disabled.
pub fn failover_enabled(db: &Database, app: &str) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);
    Ok(conn
        .query_row(
            "SELECT auto_failover_enabled FROM proxy_config WHERE app_type = ?1",
            [app],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Shared acceptance rule for model mutation and advertised picker options.
pub fn current_supports_model(db: &Database, app: &str, model: &str) -> Result<bool, AppError> {
    let Some(current) = current_provider_id(db, app) else {
        return Ok(true);
    };
    let Some(provider) = db.get_provider_by_id(&current, app)? else {
        return Ok(true);
    };
    Ok(auto_strategy::tier_models(&provider)
        .iter()
        .any(|entry| entry == model))
}

pub fn set_model(db: &Database, app: &str, model: Option<&str>) -> Result<(), AppError> {
    AppType::from_str(app)?;
    if let Some(model) = model {
        if !current_supports_model(db, app, model)? {
            return Err(AppError::Config(
                "Selected provider does not support this model".into(),
            ));
        }
    }
    migrate(db, app)?;
    auto_strategy::set_model_pref(db, app, model)
}

/// Resolve saved intent against the explicit selection. An incompatible manual
/// choice establishes its own model instead of carrying a stale preference.
pub fn effective_model(db: &Database, app: &str) -> Option<String> {
    let pref = auto_strategy::get_model_pref(db, app)?;
    let Some(current) =
        current_provider_id(db, app).and_then(|id| db.get_provider_by_id(&id, app).ok().flatten())
    else {
        return Some(pref);
    };
    if auto_strategy::tier_models(&current).contains(&pref) {
        Some(pref)
    } else {
        AppType::from_str(app).ok().and_then(|app| {
            crate::relay::provider_config::selected_model(&app, &current.settings_config)
        })
    }
}

pub fn model_for_provider(db: &Database, app: &str, provider: &Provider) -> Option<String> {
    if let Some(model) = effective_model(db, app) {
        if auto_strategy::tier_models(provider).contains(&model) {
            return Some(model);
        }
    }
    codex_tier_selected_model(app, provider)
}

/// codex 系档位的兜底模型：档位已选模型（config.toml `model`）。
///
/// 站点按请求里的模型名计费，而 codex 客户端侧的选型（Codex Desktop 的
/// 会话模型记忆/新线程默认）不是计费意图的来源——用户在 LoongPort 档位上
/// 选的模型才是。没有显式模型偏好（或偏好不被该档位服务）时，出站模型
/// 回落到档位已选模型，而不是放行客户端模型——否则会出现「选了 5.6 sol、
/// 实际按更贵的客户端默认模型计费」的静默分叉。偏好不兼容档位时的回落
/// （[`effective_model`]）本就是档位已选模型，这里是同一条规则的「无偏好」
/// 分支补齐。
///
/// 豁免与不参与的边界：
/// - **codex 官方直通**：订阅计费，模型选择权归用户，不强制。
/// - **claude**：角色映射（haiku/sonnet/opus → 档位角色模型）已在
///   `model_mapper` 对齐，这里再兜底会压平角色档。
/// - **gemini**：模型在 URL 里，body 改写触达不到。
/// - **grokbuild**：`apply_codex_upstream_model` 已自行锚定出站模型。
/// - **档位没写模型**（手工配置、问不出目录）：无从对齐，维持透传。
fn codex_tier_selected_model(app: &str, provider: &Provider) -> Option<String> {
    let app_type = AppType::from_str(app).ok()?;
    if !matches!(app_type, AppType::Codex | AppType::CodexImage) {
        return None;
    }
    if super::providers::is_codex_official_provider(provider) {
        return None;
    }
    crate::relay::provider_config::selected_model(&app_type, &provider.settings_config)
        .filter(|model| !model.trim().is_empty())
}

/// Takeover permission is separate from whether fallback is allowed.
pub fn takeover_enabled(db: &Database, app: &str) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);
    Ok(conn
        .query_row(
            "SELECT enabled FROM proxy_config WHERE app_type = ?1",
            [app],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> Provider {
        Provider::with_id(id.into(), id.into(), serde_json::json!({}), None)
    }

    #[test]
    #[serial_test::serial]
    fn blocked_tiers_round_trip_and_exclude_from_fallback() {
        let db = crate::Database::memory().unwrap();
        let provider =
            crate::provider::Provider::with_id("a".into(), "A".into(), serde_json::json!({}), None);
        db.save_provider("claude", &provider).unwrap();
        let exclusion = |blocked: &std::collections::HashSet<String>| {
            fallback_exclusion_with(&db, "claude", &provider, blocked)
        };
        assert!(blocked_tier_ids(&db, "claude").is_empty());
        assert_eq!(exclusion(&Default::default()), None);

        set_tier_blocked(&db, "claude", "a", true).unwrap();
        let blocked = blocked_tier_ids(&db, "claude");
        assert!(blocked.contains("a"));
        // 屏蔽压过其他推断性原因（模型不匹配/能力声明）——用户显式意图优先。
        assert_eq!(exclusion(&blocked), Some("blocked"));

        // 取消屏蔽恢复原状；名单按 app 隔离（codex 不受 claude 影响）。
        set_tier_blocked(&db, "claude", "a", false).unwrap();
        assert!(blocked_tier_ids(&db, "claude").is_empty());
        assert!(blocked_tier_ids(&db, "codex").is_empty());
        assert_eq!(exclusion(&Default::default()), None);

        // 未知档位拒绝写入。
        assert!(set_tier_blocked(&db, "claude", "ghost", true).is_err());
    }

    /// 链 = 用户「应用此顺序」的载荷本身：原样落库不垫底（被筛出的档位不是后备），
    /// 空列表拒绝（故障切换链至少要有一个成员）。
    #[test]
    #[serial_test::serial]
    fn set_order_stores_exactly_what_was_applied() {
        let db = crate::Database::memory().unwrap();
        for id in ["a", "b", "x"] {
            db.save_provider("claude", &provider(id)).unwrap();
        }

        set_order(&db, "claude", &["b".into(), "a".into()]).unwrap();
        assert_eq!(chain_ids(&db, "claude").unwrap(), vec!["b", "a"]);

        assert!(set_order(&db, "claude", &[]).is_err());
        assert!(
            set_order(&db, "claude", &["b".into(), "ghost".into()]).is_err(),
            "未知档位拒绝写入"
        );
    }

    /// 链未初始化时读取回落全量显示序——与启动 migrate 的全量播种等价，
    /// 「从没应用过」与「应用了全部」在读取侧同形。
    #[test]
    #[serial_test::serial]
    fn uninitialized_chain_reads_as_full_display_order() {
        let db = crate::Database::memory().unwrap();
        for id in ["a", "b"] {
            db.save_provider("claude", &provider(id)).unwrap();
        }
        assert_eq!(chain_ids(&db, "claude").unwrap(), vec!["a", "b"]);
        assert_eq!(
            chain_providers(&db, "claude")
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    /// 上游删掉的档位以幽灵形式留在链里（选路自然跳过），用户下次应用即清理。
    #[test]
    #[serial_test::serial]
    fn deleted_tiers_linger_as_ghosts_until_next_apply() {
        let db = crate::Database::memory().unwrap();
        for id in ["a", "b"] {
            db.save_provider("claude", &provider(id)).unwrap();
        }
        set_order(&db, "claude", &["a".into(), "b".into()]).unwrap();

        db.delete_provider("claude", "b").unwrap();
        assert_eq!(
            chain_ids(&db, "claude").unwrap(),
            vec!["a", "b"],
            "幽灵留在存储链里，等用户应用清理"
        );
        assert_eq!(
            chain_providers(&db, "claude")
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec!["a"],
            "选路侧跳过幽灵"
        );
    }

    /// 新档位自动进链垫底（上游新增默认排在最后生效）；编辑更新不追加——
    /// 被用户应用出链的档位刷新后不能爬回链里；链未初始化不抢跑（migrate 播种负责）。
    #[test]
    #[serial_test::serial]
    fn new_providers_join_chain_tail_but_updates_neither_append_nor_return() {
        let db = crate::Database::memory().unwrap();
        db.save_provider("claude", &provider("a")).unwrap();
        // 链未初始化：插入不追加。
        db.save_provider("claude", &provider("b")).unwrap();
        migrate(&db, "claude").unwrap();
        assert_eq!(chain_ids(&db, "claude").unwrap(), vec!["a", "b"]);

        // 初始化后新增 → 追加到链尾。
        db.save_provider("claude", &provider("c")).unwrap();
        assert_eq!(chain_ids(&db, "claude").unwrap(), vec!["a", "b", "c"]);

        // 应用把 c 筛出链；c 再刷新（更新）→ 不回链。
        set_order(&db, "claude", &["a".into(), "b".into()]).unwrap();
        let mut refreshed = provider("c");
        refreshed.name = "C refreshed".into();
        db.save_provider("claude", &refreshed).unwrap();
        assert_eq!(
            chain_ids(&db, "claude").unwrap(),
            vec!["a", "b"],
            "链外档位编辑后不爬回链里"
        );
    }

    fn codex_tier(id: &str, model: &str) -> Provider {
        Provider::with_id(
            id.into(),
            id.into(),
            serde_json::json!({ "config": format!("model = \"{model}\"\n") }),
            None,
        )
    }

    /// codex 档位没有模型偏好时，出站模型回落到档位已选模型——
    /// 站点按请求里的模型名计费，codex 客户端侧的选型（Codex Desktop
    /// 的会话模型记忆/默认）不是计费意图的来源。
    #[test]
    #[serial_test::serial]
    fn codex_tier_without_preference_enforces_selected_model() {
        let db = crate::Database::memory().unwrap();
        let tier = codex_tier("relay", "gpt-5.6-sol");
        assert_eq!(
            model_for_provider(&db, "codex", &tier),
            Some("gpt-5.6-sol".to_string())
        );
        assert_eq!(
            model_for_provider(&db, "codex-image", &tier),
            Some("gpt-5.6-sol".to_string()),
            "生图档位同规：档位已选模型即计费契约"
        );
    }

    /// codex 官方直通豁免：订阅计费下模型选择权归用户，不强制档位模型。
    #[test]
    #[serial_test::serial]
    fn codex_official_passthrough_keeps_client_choice() {
        let db = crate::Database::memory().unwrap();
        let mut official = codex_tier(crate::database::CODEX_OFFICIAL_PROVIDER_ID, "gpt-5.6-sol");
        official.category = Some("official".into());
        assert_eq!(model_for_provider(&db, "codex", &official), None);
    }

    /// 档位没写模型（手工配置、问不出目录）→ 无从对齐，维持客户端模型透传。
    #[test]
    #[serial_test::serial]
    fn codex_tier_without_selected_model_stays_verbatim() {
        let db = crate::Database::memory().unwrap();
        let tier = Provider::with_id(
            "relay".into(),
            "relay".into(),
            serde_json::json!({ "config": "model_provider = \"custom\"\n" }),
            None,
        );
        assert_eq!(model_for_provider(&db, "codex", &tier), None);
    }

    /// claude / gemini 不参与 codex 兜底：claude 的角色映射（haiku/sonnet/opus →
    /// 档位角色模型）已在 model_mapper 对齐，这里再兜底会压平角色档；gemini
    /// 的模型在 URL 里，body 改写触达不到。
    #[test]
    #[serial_test::serial]
    fn claude_and_gemini_without_preference_keep_client_choice() {
        let db = crate::Database::memory().unwrap();
        let claude = Provider::with_id(
            "relay".into(),
            "relay".into(),
            serde_json::json!({ "env": { "ANTHROPIC_MODEL": "claude-opus-5" } }),
            None,
        );
        assert_eq!(model_for_provider(&db, "claude", &claude), None);
        let gemini = Provider::with_id(
            "relay".into(),
            "relay".into(),
            serde_json::json!({ "env": { "GEMINI_MODEL": "gemini-3.5-flash" } }),
            None,
        );
        assert_eq!(model_for_provider(&db, "gemini", &gemini), None);
    }

    /// 显式模型偏好照旧优先；偏好不被目标档位服务时（故障切换到服务
    /// 其它模型的档位），回落到目标档位自己的已选模型，而不是放行
    /// 客户端模型。
    #[test]
    #[serial_test::serial]
    fn preference_wins_and_failover_tier_falls_back_to_own_selection() {
        let db = crate::Database::memory().unwrap();
        let mut pref_tier = codex_tier("current", "gpt-5.6-sol");
        pref_tier.settings_config["modelCatalog"] =
            serde_json::json!({ "models": [ { "model": "gpt-5.6-luna" } ] });
        auto_strategy::set_model_pref(&db, "codex", Some("gpt-5.6-luna")).unwrap();
        assert_eq!(
            model_for_provider(&db, "codex", &pref_tier),
            Some("gpt-5.6-luna".to_string()),
            "偏好被档位服务时照旧优先"
        );

        let failover_target = codex_tier("other", "gpt-5.6-terra");
        assert_eq!(
            model_for_provider(&db, "codex", &failover_target),
            Some("gpt-5.6-terra".to_string()),
            "目标档位不服务偏好 → 回落目标档位已选模型"
        );
    }
}
