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

pub fn ordered_providers(db: &Database, app: &str) -> Result<Vec<Provider>, AppError> {
    AppType::from_str(app)?;
    let order: Vec<String> = db
        .get_setting(&priority_key(app))?
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|e| AppError::Config(e.to_string()))?
        .unwrap_or_default();
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

/// Read local selection without the legacy getter's stale-setting cleanup.
pub fn current_provider_id(db: &Database, app: &str) -> Option<String> {
    let app_type = AppType::from_str(app).ok()?;
    crate::settings::get_current_provider(&app_type)
        .filter(|id| db.get_provider_by_id(id, app).ok().flatten().is_some())
        .or_else(|| db.get_current_provider(app).ok().flatten())
}

/// A static exclusion shared by route selection and its presentation.
pub fn fallback_exclusion(db: &Database, app: &str, provider: &Provider) -> Option<&'static str> {
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

pub fn set_order(db: &Database, app: &str, ids: &[String]) -> Result<(), AppError> {
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
    let mut order = ids.to_vec();
    order.extend(
        providers
            .into_iter()
            .map(|p| p.id)
            .filter(|id| seen.insert(id.clone())),
    );
    db.set_setting(
        &priority_key(app),
        &serde_json::to_string(&order).map_err(|e| AppError::Config(e.to_string()))?,
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
        AppType::from_str(app)
            .ok()
            .and_then(|app| crate::relay::provision::selected_model(&app, &current.settings_config))
    }
}

pub fn model_for_provider(db: &Database, app: &str, provider: &Provider) -> Option<String> {
    effective_model(db, app).filter(|model| auto_strategy::tier_models(provider).contains(model))
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
