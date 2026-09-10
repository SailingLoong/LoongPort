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
/// - `tier_keys`：档位 id → 缓存键 `(origin, account_id)`（https 端点才参与，
///   http/本地/无 sk 自然跳过 —— 单元测试零网络）；
/// - `site_keys`：缓存键 → 该账号第一把 sk（同账号多档共用一份钱包余额；
///   同站**不同账号**是不同键，见 [`balance::SiteAccountKey`]）。
///
/// 档位没有账号归属（`meta.loongport_account_id` 为 `None`）就不进这条链：
/// vendor 档的账号身份是字符串（另一套体系），未记账号的老托管档则定位不了
/// 该显示谁的钱包 —— 宁可不查，不能挂到别的账号头上。
///
/// 网络查询本身在 [`balance::spawn_stale_refresh`]（后台 SWR）。
pub(crate) fn tier_site_keys(
    app_type: &str,
    tiers: &[Provider],
) -> (
    HashMap<String, balance::SiteAccountKey>,
    HashMap<balance::SiteAccountKey, String>,
) {
    let mut tier_keys = HashMap::new();
    let mut site_keys = HashMap::new();
    let app = match AppType::from_str(app_type) {
        Ok(app) => app,
        Err(_) => return (tier_keys, site_keys),
    };
    let Some(adapter) = crate::proxy::providers::get_adapter(&app) else {
        return (tier_keys, site_keys);
    };
    for tier in tiers {
        let Some(account_id) = tier.meta.as_ref().and_then(|m| m.loongport_account_id) else {
            continue;
        };
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
        let key: balance::SiteAccountKey = (origin, account_id);
        tier_keys.insert(tier.id.clone(), key.clone());
        site_keys.entry(key).or_insert(auth.api_key);
    }
    (tier_keys, site_keys)
}

/// 全部托管档（跨 app）的站点查询材料。模式无关：只看「有哪些托管档」，
/// 不看用户在用省心还是自主 —— 两种模式底下是同一批站点数据。
fn collect_all_site_keys(db: &Database) -> HashMap<balance::SiteAccountKey, String> {
    let mut merged: HashMap<balance::SiteAccountKey, String> = HashMap::new();
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
        for (key, sk) in site_keys {
            merged.entry(key).or_insert(sk);
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

/// 充值窗关闭：先删该账号的缓存行（充值后旧值必错），再立即单账号补刷 ——
/// 事件驱动、单站、秒级，与关窗刷行余额（JWT 路）互补。
///
/// 只动**充值那个账号**的缓存行：同站另一个账号的钱包没变，不该跟着作废重查。
/// 行没登录（`account_id` 为 `None`）时新键控下本来就没有它的缓存行，无事可做。
pub fn refresh_after_purchase<R: tauri::Runtime>(
    db: &Arc<Database>,
    app_handle: Option<tauri::AppHandle<R>>,
    relay_site_origin: &str,
    relay_api_base: &str,
    account_id: Option<i64>,
) {
    let Some(account_id) = account_id else {
        return;
    };
    let keys: HashSet<balance::SiteAccountKey> = [relay_site_origin, relay_api_base]
        .into_iter()
        .filter_map(origin_of)
        .map(|origin| (origin, account_id))
        .collect();
    if keys.is_empty() {
        return;
    }
    balance::drop_site_cache(db, &keys);

    let wanted: HashMap<balance::SiteAccountKey, String> = collect_all_site_keys(db)
        .into_iter()
        .filter(|(key, _)| keys.contains(key))
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

    /// 带 `loongport_account_id` 归属的托管档（真档位 provision 起就带，见
    /// `ProviderMeta::loongport_account_id` 的文档）。
    fn managed_tier(id: &str, base: &str, account_id: Option<i64>) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            id.to_string(),
            serde_json::json!({
                "env": {
                    "ANTHROPIC_BASE_URL": base,
                    "ANTHROPIC_AUTH_TOKEN": "sk-test-not-a-real-key",
                }
            }),
            None,
        );
        provider.meta = Some(crate::provider::ProviderMeta {
            loongport_account_id: account_id,
            ..Default::default()
        });
        provider
    }

    /// 冷启补刷的站点收集：同账号多档去重、同站不同账号分开、只认托管档、
    /// http 端点不参与、无账号归属不进链（零网络）。
    #[test]
    fn collect_all_site_keys_dedupes_and_filters() {
        let db = Database::memory().unwrap();
        let tier = |provider: Provider, app: &str| {
            db.save_provider(app, &provider).unwrap();
        };
        // 同站同账号两档（不同分组）：去重成一个键
        tier(
            managed_tier(
                &crate::relay::provision::provider_id_for("https://a.example", Some(1), 1),
                "https://a.example",
                Some(1),
            ),
            "claude",
        );
        tier(
            managed_tier(
                &crate::relay::provision::provider_id_for("https://a.example", Some(1), 2),
                "https://a.example/v1",
                Some(1),
            ),
            "claude",
        );
        // 同站另一个账号的档：独立成键，不许被同站去重吞掉
        tier(
            managed_tier(
                &crate::relay::provision::provider_id_for("https://a.example", Some(2), 1),
                "https://a.example",
                Some(2),
            ),
            "claude",
        );
        // 非托管档与 http 档：不进收集
        tier(
            managed_tier("manual-tier", "https://b.example", Some(1)),
            "claude",
        );
        tier(
            managed_tier(
                &crate::relay::provision::provider_id_for("http://c.example", Some(1), 3),
                "http://c.example",
                Some(1),
            ),
            "claude",
        );
        // 无账号归属的托管档：定位不了钱包归谁，不进链
        tier(
            managed_tier(
                &crate::relay::provision::provider_id_for("https://d.example", Some(1), 1),
                "https://d.example",
                None,
            ),
            "claude",
        );

        let wanted = collect_all_site_keys(&db);
        assert_eq!(
            wanted.len(),
            2,
            "只留 a.example 的两个账号（同账号去重、http/手工/无归属排除）"
        );
        assert!(wanted.contains_key(&("https://a.example".to_string(), 1)));
        assert!(wanted.contains_key(&("https://a.example".to_string(), 2)));
    }

    /// 充值关窗编排：旧缓存行被删、该账号被单账号补刷（不可达站最终落负缓存）。
    /// 充值后旧值必然错 —— 删而不刷会让看板显示"—"直到下次启动，所以两步都要。
    #[tokio::test]
    async fn refresh_after_purchase_drops_and_refetches_that_site() {
        // A local TLS peer closes connections immediately; no external DNS dependency.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let db = Arc::new(Database::memory().unwrap());
        // Two accounts share the same local failing endpoint.
        for (account_id, group_id) in [(1, 1), (2, 1)] {
            db.save_provider(
                "claude",
                &managed_tier(
                    &crate::relay::provision::provider_id_for(&origin, Some(account_id), group_id),
                    &origin,
                    Some(account_id),
                ),
            )
            .unwrap();
        }
        // 充值前的旧值（不同 fetched_at，用远过去时间避开与刷新结果撞值）；
        // 顺带给**没充值的那个账号**放一条新鲜缓存，守「只动充值账号」的边界。
        crate::relay::balance::upsert_site_balance(&db, &(origin.clone(), 1), (Some(1.23), 100))
            .unwrap();
        crate::relay::balance::upsert_site_balance(&db, &(origin.clone(), 2), (Some(9.99), 100))
            .unwrap();

        crate::services::site_balance_refresh::refresh_after_purchase(
            &db,
            None::<tauri::AppHandle>,
            &origin,
            &origin,
            Some(1),
        );

        for _ in 0..250 {
            let entry = crate::relay::balance::cached_site_balances(&db)
                .get(&(origin.clone(), 1))
                .copied();
            // 旧值(1.23, 100)被删，最终落到补刷结果（负缓存、新 fetched_at）
            if let Some((balance, fetched_at)) = entry {
                assert_eq!(balance, None, "不可达站补刷结果 = 负缓存");
                assert!(fetched_at > 1_000, "必须是补刷写的新行，不是充值前旧值");
                assert_eq!(
                    crate::relay::balance::cached_site_balances(&db).get(&(origin.clone(), 2)),
                    Some(&(Some(9.99), 100)),
                    "充值只失效充值账号的缓存，同站另一账号不动"
                );
                server.abort();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("充值关窗没有完成「删旧值 + 单账号补刷」");
    }
}
