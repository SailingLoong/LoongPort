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
    relay: &creds::RelayAccount,
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
/// ## 为什么返回 [`UsageResult`] 而不是 `sub2api::Balance`
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
/// ## 读路径接进 `site_balance_cache`（2026-09-07 收口，修「没改透」）
///
/// 站点余额的唯一事实源是 [`crate::relay::balance`] 的缓存（看板 #283/#286 起
/// 已走它）；本命令此前**每打一次都同步走全链路网络** —— 同一个事实两条读路径，
/// 开页每行转圈、跨境抖动直接渲染成「查询失败」。现在同一张表：
///
/// - 缓存新鲜（TTL 内，含负缓存）→ **秒回缓存值**，零网络；
/// - 缓存过期但有值 → 立即回旧值 + 踢后台单飞刷新（完成发
///   `SITE_BALANCES_UPDATED`，前端失效重读）—— SWR；
/// - 没进过缓存 / `force`（手动刷新按钮）→ 走下面的全链路解析，**结果写回缓存**
///   （正/负都写），行路径从「旁路」变成「又一个写入方」。
///
/// 对账快照采样只在真查那条路上落（缓存命中没有新信息，不采样）。
#[tauri::command]
pub async fn relay_balance(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    force: Option<bool>,
) -> Result<balance::RowBalanceResult, String> {
    relay_balance_impl(&app_handle, relay_id, force.unwrap_or(false))
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn relay_balance_impl<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    relay_id: i64,
    force: bool,
) -> Result<balance::RowBalanceResult, AppError> {
    let (relay, base_url, api_keys) = {
        let state = app_handle.state::<AppState>();
        let relay = with_conn(&state, |conn| creds::get(conn, relay_id))?
            .ok_or_else(|| AppError::Config(format!("找不到 id 为 {relay_id} 的中转站")))?;
        let (base_url, api_keys) = relay_balance_inputs(&state, &relay);
        (relay, base_url, api_keys)
    };

    // 缓存键 = (site_origin, account_id)。未登录（`account_id == None`）的行进不了
    // 缓存：钱包事实归账号所有，没有账号维度就只能走真查，绝不能挂到别的账号
    // 条目上 —— 同站两个账号曾经在这里互相顶掉对方的余额。
    if !force {
        if let Some(account_id) = relay.account_id {
            let state = app_handle.state::<AppState>();
            let key: balance::SiteAccountKey = (relay.site_origin.clone(), account_id);
            if let Some(entry) = balance::cached_site_balance(&state.db, &key) {
                let fresh =
                    chrono::Utc::now().timestamp() - entry.1 <= balance::SITE_BALANCE_TTL_SECS;
                if !fresh {
                    // 过期：回旧值 + 后台刷（单飞/预算/事件都在那条链里；只用 sk，
                    // 不碰登录态 —— 充值窗口的 cookie 独占权不受影响）。
                    if let Some(sk) = api_keys.first() {
                        balance::spawn_stale_refresh(
                            state.db.clone(),
                            Some(app_handle.clone()),
                            std::collections::HashMap::from([(key, sk.clone())]),
                        );
                    }
                }
                return Ok(balance::cached_row_balance_result(&entry));
            }
        }
    }

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

    {
        // 真查的结果写回缓存（正/负都写）—— 行路径与看板/后台刷新写同一张表，
        // 这是「一个事实一个 owner」的落点。未登录行没有账号维度，只采样不缓存。
        // 快照采样保持只挂真查。
        let state = app_handle.state::<AppState>();
        let cached_value = usage
            .data
            .as_ref()
            .and_then(|items| items.first())
            .and_then(|item| item.remaining);
        if let Some(account_id) = relay.account_id {
            if let Err(e) = balance::upsert_site_balance(
                &state.db,
                &(relay.site_origin.clone(), account_id),
                (cached_value, chrono::Utc::now().timestamp()),
            ) {
                log::warn!("[site-balance] 行查询写缓存失败: {e}");
            }
        }
        reconcile::capture_balance_snapshot(&state.db, relay_id, &usage);
    }

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

    /// 行级余额读路径接进 site_balance_cache 后的行为闸：
    /// TTL 内第二次查询**秒回缓存**（mock 服务只被打一次），`force` 旁路缓存直查。
    /// 这条守的是「开页不再每行转圈 / 抖动不再渲染成查询失败」的用户可见语义。
    #[tokio::test]
    async fn row_balance_serves_cache_within_ttl_and_force_bypasses() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_router = hits.clone();
        let counting_router = {
            use axum::{routing::get, Json, Router};
            let balance = serde_json::json!({
                "code": 0, "message": "success",
                "data": { "id": 7, "username": "u", "email": "u@example.com",
                          "balance": 3.25, "frozen_balance": 0.0 }
            });
            Router::new().route(
                "/api/v1/user/profile",
                get(move || async move {
                    hits_for_router.fetch_add(1, Ordering::SeqCst);
                    Json(balance)
                }),
            )
        };
        let (origin, _server) = spawn_balance_server(counting_router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        // 第一次：缓存空 → 走真查（网络一次），结果写缓存。
        let first = relay_balance_impl(app.handle(), relay_id, false)
            .await
            .expect("first resolve");
        assert!(first.usage.success);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 第二次：TTL 内 → 秒回缓存，网络零新增。
        let second = relay_balance_impl(app.handle(), relay_id, false)
            .await
            .expect("cached read");
        assert!(second.usage.success, "缓存正条目也要能拼出成功展示");
        assert_eq!(
            second
                .usage
                .data
                .as_ref()
                .and_then(|i| i.first())
                .and_then(|i| i.remaining),
            Some(3.25),
            "缓存读回同一个数"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "TTL 内不该再打网络");

        // force：手动刷新语义 → 旁路缓存直查（网络第二次）。
        let forced = relay_balance_impl(app.handle(), relay_id, true)
            .await
            .expect("forced resolve");
        assert!(forced.usage.success);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "force 必须真的重新查");
    }

    /// ⭐ 同站双账号端到端：A 行的余额查询不能命中、也不能覆盖 B 行的缓存
    /// （缓存键 = 站点 + 账号）。修复前两行共用一个缓存槽，后查的账号把先查的
    /// 顶掉 —— 界面上两行显示同一个数。
    #[tokio::test]
    async fn same_site_two_accounts_do_not_share_cache_entries() {
        // mock 按 bearer token 回不同余额：token-a → 1.0，token-b → 2.0
        let router = {
            use axum::http::HeaderMap;
            use axum::routing::get;
            use axum::Json;
            axum::Router::new().route(
                "/api/v1/user/profile",
                get(|headers: HeaderMap| async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default();
                    let balance = if auth.contains("token-b") { 2.0 } else { 1.0 };
                    Json(serde_json::json!({
                        "code": 0, "message": "success",
                        "data": { "id": 7, "username": "u", "email": "u@example.com",
                                  "balance": balance, "frozen_balance": 0.0 }
                    }))
                }),
            )
        };
        let (origin, _server) = spawn_balance_server(router).await;

        // 同站两行两个账号（7 / token-a，8 / token-b）
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("内存库"));
        let (row_a, row_b) = {
            let conn = db.conn.lock().expect("锁内存库");
            let row_a = creds::save_site_with_backend(
                &conn,
                &origin,
                "A",
                &origin,
                discovery::BackendKind::Sub2Api,
            )
            .expect("建 A 行");
            creds::save_credentials(
                &conn,
                row_a,
                creds::AccountIdentity {
                    id: 7,
                    label: "A",
                    login_identifier: "a",
                },
                "token-a",
                Some("refresh-a"),
                None,
                creds::SessionEnvironment::default(),
            )
            .expect("存 A 登录态");
            let row_b = creds::save_site_with_backend(
                &conn,
                &origin,
                "B",
                &origin,
                discovery::BackendKind::Sub2Api,
            )
            .expect("建 B 行");
            creds::save_credentials(
                &conn,
                row_b,
                creds::AccountIdentity {
                    id: 8,
                    label: "B",
                    login_identifier: "b",
                },
                "token-b",
                Some("refresh-b"),
                None,
                creds::SessionEnvironment::default(),
            )
            .expect("存 B 登录态");
            (row_a, row_b)
        };
        let app = tauri::test::mock_builder()
            .manage(AppState::new(db))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");

        let remaining = |result: &balance::RowBalanceResult| {
            result
                .usage
                .data
                .as_ref()
                .and_then(|items| items.first())
                .and_then(|item| item.remaining)
                .expect("成功路径必有余额")
        };

        // A 首查：真查落缓存（origin, 7) = 1.0
        let a_first = relay_balance_impl(app.handle(), row_a, false)
            .await
            .expect("A 首查");
        assert_eq!(remaining(&a_first), 1.0);

        // B 查：不得命中 A 的缓存，必须真查到自己的 2.0（修复前这里秒回 A 的 1.0）
        let b_first = relay_balance_impl(app.handle(), row_b, false)
            .await
            .expect("B 查询");
        assert_eq!(
            remaining(&b_first),
            2.0,
            "B 行必须查到 B 账号的钱包，不能读到 A 的缓存"
        );

        // A 再查：仍读自己的 1.0 —— B 的写入没有顶掉 A 的缓存行
        let a_second = relay_balance_impl(app.handle(), row_a, false)
            .await
            .expect("A 再查");
        assert_eq!(
            remaining(&a_second),
            1.0,
            "B 的查询/写缓存不得影响 A 的缓存行"
        );
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

        let result = relay_balance_impl(app.handle(), relay_id, false)
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

        let result = relay_balance_impl(app.handle(), relay_id, false)
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

        let result = relay_balance_impl(app.handle(), relay_id, false)
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
