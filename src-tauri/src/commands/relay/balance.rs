//! 余额查询命令（缓存与单飞刷新在 relay::balance）。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::balance;
use crate::relay::provision;

/// 一行名下**全部托管档位**里的 base_url 与 sk。
///
/// 归属判据走 [`belongs_to_relay`]（严格版：未登录的行不认别人账号的档位），但
/// **跨全部 app 扫** —— 同一行的档位可能只挂在某一个 CLI 下（用户只给 codex 生成过 sk），
/// 按单个 app 查会在别的 app 上空手而归，让余额白白落到下一条路。
///
/// 顺序不稳定不要紧：调用方（[`crate::relay::balance::resolve`]）是并发试完取第一个
/// 拿到结果的，同一行的每把 sk 问出的钱包余额是同一个账户的同一个数。
pub(crate) fn relay_balance_inputs(
    state: &AppState,
    relay: &creds::Relay,
) -> (String, Vec<String>) {
    let mut base_url = None;
    let mut keys: Vec<String> = Vec::new();
    for app_type in AppType::all() {
        let Ok(providers) = ProviderService::list(state, app_type.clone()) else {
            continue;
        };
        for provider in providers.values() {
            if !belongs_to_relay(provider, &relay.site_origin, relay.account_id) {
                continue;
            }
            if let Some(sk) = provision::extract_api_key(&provider.settings_config, &app_type) {
                if base_url.is_none() {
                    base_url = crate::proxy::providers::get_adapter(&app_type)
                        .and_then(|adapter| adapter.extract_base_url(provider).ok());
                }
                if !sk.trim().is_empty() && !keys.contains(&sk) {
                    keys.push(sk);
                }
            }
        }
    }
    (base_url.unwrap_or_default(), keys)
}

/// 余额。`relay_id` 指定查**哪一行**的。
///
/// 与 [`relay_login`] / [`relay_refresh`] 同一套纪律：显式指定查不到就报错，绝不
/// 回落到其它站点 —— 那会把 B 的余额显示在 A 那一行上，比报错更糟。
///
/// ## 一行一次请求是安全的
///
/// `/user/profile` **没挂 `Heavy()`**，只吃 `panelRateLimiter.Global()`
/// （sub2api 默认 `UserRPM = 240/分钟`，按 user_id 计数）—— 而且不同中转站行往往是
/// **不同用户**，各记各的额度。N 行各打一次远远碰不到限流。
///
/// ## 为什么返回 [`UsageResult`] 而不是 `api::Balance`
///
/// 这是本轮最主要的收敛（全局准则 §1.4）。原来中转站行回 `{balance, frozenBalance}`
/// 数字、官网行回**后端已格式化好的字符串** `"¥547.08"` —— 同一个事实两套契约，
/// 于是前端也就有两份余额 state、两个 effect、两处渲染。改成两类行都回上游那个
/// [`UsageResult`] 之后，前端只剩一个 hook 一个组件，还顺带白拿了 provider 页
/// 那套用量条（上次查询时间 + 手动刷新按钮）。
///
/// ## **不走 [`usable_relay`]，因此登录态过期也能查**
///
/// 这是本轮的目的本身。`usable_relay` 会校验登录态、过期就报错 —— 而 sk 是独立凭据，
/// 登录态过期时它照样能调用。所以这里只读那一行的记录（[`creds::get`]），把
/// 「有没有可用登录态」交给 [`crate::relay::balance::resolve`] 的第 3 步自己判：
/// 前两步不需要登录态，第 3 步需要，语义落在那一步里而不是拦在门口。
///
/// **不返回 `Err`**（除了「这一行不存在」）：三条路都失败时回 `success:false`，
/// 前端才有失败态可渲染、有刷新按钮可点。见 [`crate::relay::balance`] 模块文档。
#[tauri::command]
pub async fn relay_balance(
    app_handle: tauri::AppHandle,
    relay_id: i64,
) -> Result<balance::RowBalanceResult, String> {
    relay_balance_impl(&app_handle, relay_id)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn relay_balance_impl<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
) -> Result<balance::RowBalanceResult, AppError> {
    let (relay, base_url, api_keys) = {
        let state = app_handle.state::<AppState>();
        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {relay_id} 的中转站")))?;
        let (base_url, api_keys) = relay_balance_inputs(&state, &relay);
        (relay, base_url, api_keys)
    };

    let usage = balance::resolve(
        balance::BalanceQuery {
            site_origin: &relay.site_origin,
            base_url: &base_url,
            api_keys: &api_keys,
        },
        // 登录态还在才给第 3 步（JWT 路）。空 token 就别让它白打一个必定 401 的请求。
        if relay.auth_token.trim().is_empty() {
            balance::SessionFallback::None
        } else {
            balance::SessionFallback::Relay(&relay)
        },
    )
    .await
    .usage;

    // 余额快照是扣费对账的旁路采样，挂在成功解析之后：写入条件与失败兜底都在
    // `reconcile::capture_balance_snapshot` 里，这里只出一条调用，不拖垮余额显示。
    reconcile::capture_balance_snapshot(&app_handle.state::<AppState>().db, relay_id, &usage);

    Ok(balance::row_balance_result(usage, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

    fn profile_router(balance: serde_json::Value) -> axum::Router {
        use axum::{routing::get, Json, Router};
        Router::new().route(
            "/api/v1/user/profile",
            get(move || async move { Json(balance) }),
        )
    }

    #[tokio::test]
    async fn relay_balance_writes_one_snapshot_on_successful_resolve() {
        let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": {
                "id": 7,
                "username": "Sub User",
                "email": "sub@example.com",
                "balance": 12.5,
                "frozen_balance": 0.0
            }
        })))
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("成功解析必须 Ok");

        assert!(result.usage.success, "{:?}", result.usage.error);
        let state = app.state::<AppState>();
        let rows = state
            .db
            .list_balance_snapshots(relay_id, None)
            .expect("读快照");
        assert_eq!(rows.len(), 1, "成功路径恰好落一条快照");
        assert_eq!(rows[0].balance_usd, 12.5);
        assert_eq!(rows[0].source, "balance_query");
        assert_eq!(rows[0].relay_id, relay_id);
        server.abort();
    }

    #[tokio::test]
    async fn relay_balance_writes_no_snapshot_when_resolve_fails() {
        use axum::{http::StatusCode, routing::get, Router};
        let router = Router::new().route(
            "/api/v1/user/profile",
            get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let (origin, server) = spawn_balance_server(router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("失败路回 success:false，仍是 Ok");

        assert!(!result.usage.success);
        let state = app.state::<AppState>();
        assert_eq!(
            state
                .db
                .list_balance_snapshots(relay_id, None)
                .expect("读快照")
                .len(),
            0,
            "三路全败不能落快照"
        );
        server.abort();
    }

    /// ⭐ 快照写入失败绝不能拖垮余额显示（对账是旁路能力，plan §三.2）。
    #[tokio::test]
    async fn relay_balance_returns_balance_even_when_snapshot_insert_fails() {
        let (origin, server) = spawn_balance_server(profile_router(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": {
                "id": 7,
                "username": "Sub User",
                "email": "sub@example.com",
                "balance": 9.9,
                "frozen_balance": 0.0
            }
        })))
        .await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);
        {
            let state = app.state::<AppState>();
            let conn = state.db.conn.lock().expect("lock memory database");
            conn.execute("DROP TABLE relay_balance_snapshots", [])
                .expect("删表制造写入失败");
        }

        let result = relay_balance_impl(app.handle(), relay_id)
            .await
            .expect("快照写入失败时余额命令仍必须 Ok");
        assert!(result.usage.success);
        assert_eq!(
            result
                .usage
                .data
                .as_ref()
                .and_then(|items| items.first())
                .and_then(|item| item.remaining),
            Some(9.9)
        );
        server.abort();
    }
}
