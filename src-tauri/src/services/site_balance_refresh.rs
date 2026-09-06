//! 站点余额刷新的**编排层**（模式无关）。
//!
//! 触发点与视图/模式解耦（2026-09-06 用户定的原则）：用户选省心还是自主，
//! 与底层的余额/模型等数据刷新本无关联 —— 刷新是数据层自己的事，唯一的
//! 合法触发是「没数据时补一次」这类与模式无关的时刻。看板等视图**纯读**
//! [`crate::relay::balance::cached_site_balances`]，不驱动刷新。
//!
//! 两个触发点（都不是 interval，与「余额禁轮询」决策相容，红线论证见
//! [`crate::relay::balance::spawn_stale_refresh`] 的文档）：
//! 1. **应用启动冷启补刷**（[`startup_kick`]，lib.rs 挂）：启动后一次性把
//!    stale/缺缓存的站刷齐 —— 用户点进任何视图时缓存已就绪；
//! 2. **充值窗关闭失效重刷**（[`refresh_after_purchase`]，关窗事件旁路）：
//!    充值是余额真正变化的时刻，属「零新增路径」的事件驱动采样。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use std::str::FromStr;

use crate::app_config::AppType;
use crate::database::Database;
use crate::provider::Provider;
use crate::relay::balance;

/// 从 base_url 取 `scheme://authority`（`https://site/v1` → `https://site`）。
pub(crate) fn origin_of(base_url: &str) -> Option<String> {
    let (scheme, rest) = base_url.split_once("://")?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// 档位 → 站点余额查询材料的解析（纯本地，无网络）：
/// - `tier_origins`：档位 id → 站点 origin（https 端点才参与，http/本地/无 sk
///   自然跳过 —— 单元测试零网络）；
/// - `site_keys`：origin → 该站第一把 sk（同站多档共用一份站点余额）。
///
/// 网络查询本身在 [`balance::spawn_stale_refresh`]（后台 SWR）。
pub(crate) fn tier_site_keys(
    app_type: &str,
    tiers: &[Provider],
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut tier_origins = HashMap::new();
    let mut site_keys = HashMap::new();
    let app = match AppType::from_str(app_type) {
        Ok(app) => app,
        Err(_) => return (tier_origins, site_keys),
    };
    let Some(adapter) = crate::proxy::providers::get_adapter(&app) else {
        return (tier_origins, site_keys);
    };
    for tier in tiers {
        let (base, auth) = match (adapter.extract_base_url(tier), adapter.extract_auth(tier)) {
            (Ok(base), Some(auth)) => (base, auth),
            _ => continue,
        };
        let Some(origin) = origin_of(&base) else {
            continue;
        };
        if !origin.starts_with("https://") {
            continue;
        }
        tier_origins.insert(tier.id.clone(), origin.clone());
        site_keys.entry(origin).or_insert(auth.api_key);
    }
    (tier_origins, site_keys)
}

/// 全部托管档（跨 app）的站点查询材料。模式无关：只看「有哪些托管档」，
/// 不看用户在用省心还是自主 —— 两种模式底下是同一批站点数据。
fn collect_all_site_keys(db: &Database) -> HashMap<String, String> {
    let mut merged: HashMap<String, String> = HashMap::new();
    for app in AppType::all() {
        let Ok(providers) = db.get_all_providers(app.as_str()) else {
            continue;
        };
        let tiers: Vec<Provider> = providers
            .into_values()
            .filter(|p| crate::relay::is_managed(&p.id))
            .collect();
        if tiers.is_empty() {
            continue;
        }
        let (_, site_keys) = tier_site_keys(app.as_str(), &tiers);
        for (origin, key) in site_keys {
            merged.entry(origin).or_insert(key);
        }
    }
    merged
}

/// 应用启动冷启补刷：stale/缺缓存的站交给后台单飞刷新（TTL 内的缓存不动，
/// 所以日常重启也只是把过期的补齐，不是全量重查）。
pub fn startup_kick<R: tauri::Runtime>(
    db: &Arc<Database>,
    app_handle: Option<tauri::AppHandle<R>>,
) {
    let wanted = collect_all_site_keys(db);
    balance::spawn_stale_refresh(db.clone(), app_handle, wanted);
}

/// 充值窗关闭：先删该站缓存行（充值后旧值必错），再立即单站补刷 ——
/// 事件驱动、单站、秒级，与关窗刷行余额（JWT 路）互补。
pub fn refresh_after_purchase<R: tauri::Runtime>(
    db: &Arc<Database>,
    app_handle: Option<tauri::AppHandle<R>>,
    relay_site_origin: &str,
    relay_api_base: &str,
) {
    let origins: HashSet<String> = [relay_site_origin, relay_api_base]
        .into_iter()
        .filter_map(origin_of)
        .collect();
    if origins.is_empty() {
        return;
    }
    balance::drop_site_cache(db, &origins);

    let wanted: HashMap<String, String> = collect_all_site_keys(db)
        .into_iter()
        .filter(|(origin, _)| origins.contains(origin))
        .collect();
    if wanted.is_empty() {
        return;
    }
    balance::spawn_stale_refresh(db.clone(), app_handle, wanted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_of_strips_paths() {
        assert_eq!(
            origin_of("https://api.example/v1"),
            Some("https://api.example".to_string())
        );
        assert_eq!(
            origin_of("https://site.example"),
            Some("https://site.example".to_string())
        );
        assert_eq!(origin_of("note-a-url"), None);
        assert_eq!(origin_of("https://"), None);
    }

    /// 冷启补刷的站点收集：同站多档去重、只认托管档、http 端点不参与（零网络）。
    #[test]
    fn collect_all_site_keys_dedupes_and_filters() {
        let db = Database::memory().unwrap();
        let tier = |id: &str, app: &str, base: &str| {
            db.save_provider(
                app,
                &Provider::with_id(
                    id.to_string(),
                    id.to_string(),
                    serde_json::json!({
                        "env": {
                            "ANTHROPIC_BASE_URL": base,
                            "ANTHROPIC_AUTH_TOKEN": "sk-test-not-a-real-key",
                        }
                    }),
                    None,
                ),
            )
            .unwrap();
        };
        // 同站两档（不同分组）：origin 去重成一把 key
        tier(
            &crate::relay::provision::provider_id_for("https://a.example", Some(1), 1),
            "claude",
            "https://a.example",
        );
        tier(
            &crate::relay::provision::provider_id_for("https://a.example", Some(1), 2),
            "claude",
            "https://a.example/v1",
        );
        // 非托管档与 http 档：不进收集
        tier("manual-tier", "claude", "https://b.example");
        tier(
            &crate::relay::provision::provider_id_for("http://c.example", Some(1), 3),
            "claude",
            "http://c.example",
        );

        let wanted = collect_all_site_keys(&db);
        assert_eq!(wanted.len(), 1, "只留 a.example（同站去重、http/手工排除）");
        assert!(wanted.contains_key("https://a.example"));
    }

    /// 充值关窗编排：旧缓存行被删、该站被单站补刷（不可达站最终落负缓存）。
    /// 充值后旧值必然错 —— 删而不刷会让看板显示"—"直到下次启动，所以两步都要。
    #[tokio::test]
    async fn refresh_after_purchase_drops_and_refetches_that_site() {
        let db = Arc::new(Database::memory().unwrap());
        // 该站一个托管档（https、不可达 —— .example 保留域 DNS 快速失败）
        let id = crate::relay::provision::provider_id_for("https://gone.example", Some(1), 1);
        db.save_provider(
            "claude",
            &Provider::with_id(
                id,
                "充值站".to_string(),
                serde_json::json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://gone.example",
                        "ANTHROPIC_AUTH_TOKEN": "sk-test-not-a-real-key",
                    }
                }),
                None,
            ),
        )
        .unwrap();
        // 充值前的旧值（不同 fetched_at，用远过去时间避开与刷新结果撞值）
        crate::relay::balance::upsert_site_balance(&db, "https://gone.example", (Some(1.23), 100))
            .unwrap();

        crate::services::site_balance_refresh::refresh_after_purchase(
            &db,
            None::<tauri::AppHandle>,
            "https://panel.gone.example",
            "https://gone.example",
        );

        for _ in 0..250 {
            let entry = crate::relay::balance::cached_site_balances(&db)
                .get("https://gone.example")
                .copied();
            // 旧值(1.23, 100)被删，最终落到补刷结果（负缓存、新 fetched_at）
            if let Some((balance, fetched_at)) = entry {
                assert_eq!(balance, None, "不可达站补刷结果 = 负缓存");
                assert!(fetched_at > 1_000, "必须是补刷写的新行，不是充值前旧值");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("充值关窗没有完成「删旧值 + 单站补刷」");
    }
}
