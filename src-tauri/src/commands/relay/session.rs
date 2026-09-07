//! 行级「刷新」族命令：探活、provision 重拉、余额刷新与后台定价同步。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::balance;

/// 加站弹窗需要的后端状态。
///
/// ## 为什么只剩两个字段（2026-08-04 收缩）
///
/// 原来它有 9 个字段，服务的是已删的 LoongPort 独立页那个**单站视图**
/// （顶部显示「当前站是 X、登录的是 Y、已过期了没、有几个档位」）。中转站行现在
/// 每行各显示自己的状态、数据走 [`relay_list_relays`]，那个「当前站」的概念
/// 连带消失 ⇒ 那 7 个字段前端一个都不读了。
///
/// 其中 `tier_count` 还有实际成本（遍历整个 provider 表数托管项），
/// 而这条命令是**首屏渲染要等的东西**。
///
/// ⚠️ 删的是**没有消费者**的字段，不是「暂时没用」的字段 —— 将来真要「当前站」
/// 这个概念时该重新想清楚它的语义（多行并列的界面里「当前」指什么），而不是
/// 留着这几个没人读的字段当预留。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// 域名输入框的底纹词。
    pub default_site: String,
    /// 当前是否没有任何中转站或官网账号配置，前端据此决定是否显示首次引导。
    pub should_prompt_add_site: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshNotice {
    None,
    Updated,
    UpdatedWithKeys,
    OtherPlatforms,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshFailureKind {
    KeyLimit,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshFailure {
    pub name: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<RefreshFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSummary {
    pub notice: RefreshNotice,
    pub refreshed_accounts: usize,
    pub tiers: usize,
    pub keys_created: usize,
    pub other_platform_tiers: usize,
    pub merged_providers: usize,
    pub failures: Vec<RefreshFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshedBalanceKind {
    Relay,
    Vendor,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshedBalance {
    pub kind: RefreshedBalanceKind,
    pub row_id: i64,
    pub result: balance::RowBalanceResult,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResult {
    pub summary: RefreshSummary,
    pub balances: Vec<RefreshedBalance>,
}

pub(crate) fn refresh_summary(
    app_type: &AppType,
    relay_results: Vec<(String, Result<ProvisionSummary, AppError>)>,
    vendor_results: Vec<(
        String,
        Result<
            crate::commands::vendor::VendorProvisionSummary,
            crate::commands::vendor::VendorActionError,
        >,
    )>,
) -> RefreshSummary {
    let mut refreshed_accounts = 0;
    let mut tiers = 0;
    let mut keys_created = 0;
    let mut other_platform_tiers = 0;
    let mut merged_providers = 0;
    let mut failures = Vec::new();

    for (name, result) in relay_results {
        match result {
            Ok(summary) => {
                refreshed_accounts += 1;
                let current = summary
                    .tiers
                    .iter()
                    .filter(|tier| tier.app_id == app_type.as_str())
                    .count();
                tiers += current;
                if current == 0 && summary.failures.is_empty() {
                    other_platform_tiers += summary.tiers.len();
                }
                keys_created += summary.keys_created;
                merged_providers += summary.merged_providers.len();
                failures.extend(summary.failures.into_iter().map(|failure| RefreshFailure {
                    name: failure.group_name,
                    reason: failure.reason,
                    kind: None,
                    help_url: None,
                }));
            }
            Err(error) => failures.push(RefreshFailure {
                name,
                reason: error.to_string(),
                kind: None,
                help_url: None,
            }),
        }
    }
    for (name, result) in vendor_results {
        match result {
            Ok(summary) => {
                refreshed_accounts += 1;
                let current = usize::from(
                    summary
                        .platforms
                        .iter()
                        .any(|platform| platform == app_type.as_str()),
                );
                tiers += current;
                if current == 0 {
                    other_platform_tiers += summary.platforms.len();
                }
                merged_providers += summary.merged_providers.len();
                keys_created += usize::from(summary.key_created);
            }
            Err(error) => failures.push(RefreshFailure {
                name,
                reason: error.message,
                kind: error.kind.map(|kind| match kind {
                    crate::commands::vendor::VendorActionErrorKind::KeyLimit => {
                        RefreshFailureKind::KeyLimit
                    }
                }),
                help_url: error.help_url,
            }),
        }
    }

    let notice = if refreshed_accounts == 0 {
        RefreshNotice::None
    } else if tiers == 0 && other_platform_tiers > 0 && failures.is_empty() {
        RefreshNotice::OtherPlatforms
    } else if keys_created > 0 {
        RefreshNotice::UpdatedWithKeys
    } else {
        RefreshNotice::Updated
    };
    RefreshSummary {
        notice,
        refreshed_accounts,
        tiers,
        keys_created,
        other_platform_tiers,
        merged_providers,
        failures,
    }
}

pub(crate) struct RelayRefreshOutcome {
    name: String,
    row_id: i64,
    provision: Option<Result<ProvisionSummary, AppError>>,
    balance: Result<balance::RowBalanceResult, AppError>,
    failures: Vec<RefreshFailure>,
}

pub(crate) struct VendorRefreshOutcome {
    pub name: String,
    pub row_id: i64,
    pub provision: Option<
        Result<
            crate::commands::vendor::VendorProvisionSummary,
            crate::commands::vendor::VendorActionError,
        >,
    >,
    pub balance: Result<balance::RowBalanceResult, AppError>,
}

pub(crate) fn finish_refresh_result(
    app_type: &AppType,
    relay_outcomes: Vec<RelayRefreshOutcome>,
    vendor_outcomes: Vec<VendorRefreshOutcome>,
) -> RefreshResult {
    let mut relay_results = Vec::with_capacity(relay_outcomes.len());
    let mut vendor_results = Vec::with_capacity(vendor_outcomes.len());
    let mut balances = Vec::new();
    let mut balance_failures = Vec::new();

    for outcome in relay_outcomes {
        let name = outcome.name;
        if let Some(provision) = outcome.provision {
            relay_results.push((name.clone(), provision));
        }
        balance_failures.extend(outcome.failures);
        collect_refreshed_balance(
            &mut balances,
            &mut balance_failures,
            name,
            RefreshedBalanceKind::Relay,
            outcome.row_id,
            outcome.balance,
        );
    }
    for outcome in vendor_outcomes {
        let name = outcome.name;
        if let Some(provision) = outcome.provision {
            vendor_results.push((name.clone(), provision));
        }
        collect_refreshed_balance(
            &mut balances,
            &mut balance_failures,
            name,
            RefreshedBalanceKind::Vendor,
            outcome.row_id,
            outcome.balance,
        );
    }

    let successful_balances = balances
        .iter()
        .filter(|balance| balance.result.usage.success)
        .count();
    let mut summary = refresh_summary(app_type, relay_results, vendor_results);
    summary.refreshed_accounts = summary.refreshed_accounts.max(successful_balances);
    summary.failures.extend(balance_failures);
    if matches!(summary.notice, RefreshNotice::None) && summary.refreshed_accounts > 0 {
        summary.notice = RefreshNotice::Updated;
    }

    RefreshResult { summary, balances }
}

pub(crate) fn relay_refresh_targets(
    state: &AppState,
    app_type: &AppType,
) -> Result<Vec<(i64, String, bool)>, AppError> {
    list_relays_impl(state, app_type.clone()).map(|rows| {
        rows.into_iter()
            .filter_map(|row| {
                let name = if row.account_label.is_empty() {
                    row.site_name
                } else {
                    row.account_label
                };
                (row.can_refresh || row.can_query_balance).then_some((
                    row.id,
                    name,
                    row.can_refresh,
                ))
            })
            .collect()
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RelayPricingRefreshSummary {
    pub(crate) attempted: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: Vec<(i64, String)>,
}

pub(crate) async fn refresh_due_relay_pricing_rows<F, Fut>(
    relays: Vec<creds::RelayAccount>,
    now: i64,
    interval: std::time::Duration,
    refresh: F,
) -> RelayPricingRefreshSummary
where
    F: Fn(creds::RelayAccount) -> Fut + Sync,
    Fut: Future<Output = Result<(), AppError>> + Send,
{
    use futures::StreamExt;

    let due: Vec<_> = relays
        .into_iter()
        .filter(|relay| relay.account_id.is_some() && !relay.pricing_is_fresh(now, interval))
        .collect();
    let attempted = due.len();
    let mut results = futures::stream::iter(due.into_iter().map(|relay| {
        let refresh = &refresh;
        async move {
            let relay_id = relay.id;
            (relay_id, refresh(relay).await)
        }
    }))
    .buffer_unordered(2);
    let mut summary = RelayPricingRefreshSummary {
        attempted,
        ..Default::default()
    };
    while let Some((relay_id, result)) = results.next().await {
        match result {
            Ok(()) => summary.succeeded += 1,
            Err(error) => summary.failed.push((relay_id, error.to_string())),
        }
    }
    summary
}

pub(crate) async fn refresh_due_relay_pricing(
    app_handle: tauri::AppHandle,
) -> Result<(), AppError> {
    let (relays, db) = {
        let state = app_handle.state::<AppState>();
        (with_conn(&state, creds::list)?, Arc::clone(&state.db))
    };
    let now = chrono::Utc::now().timestamp();
    let summary = refresh_due_relay_pricing_rows(
        relays,
        now,
        crate::maintenance::config::RELAY_PRICING_REFRESH_INTERVAL,
        move |relay| {
            let app_handle = app_handle.clone();
            let db = Arc::clone(&db);
            async move {
                // 倍率拉取是只读请求，走 401→续期→重试：`token_expires_at = NULL`
                // 的行靠它从「永不续期、到点暴毙」的降级态自愈。
                relay_read_with_refresh_retry(&app_handle, relay.id, |site_account| {
                    // 闭包要能调两次（原请求 + 重试），future 各自持有克隆出来的 Arc。
                    let db = Arc::clone(&db);
                    async move {
                        let updates = pricing::fetch_rate_updates(&site_account).await?;
                        pricing::apply_rate_updates(&db, &updates)?;
                        let conn = db.conn.lock().map_err(|error| {
                            AppError::Database(format!("获取数据库连接失败: {error}"))
                        })?;
                        creds::mark_pricing_synced(
                            &conn,
                            site_account.id,
                            chrono::Utc::now().timestamp(),
                        )
                    }
                })
                .await
            }
        },
    )
    .await;

    for (relay_id, error) in &summary.failed {
        log::warn!("中转站 #{relay_id} 后台倍率刷新失败: {error}");
    }
    log::info!(
        "中转站后台倍率刷新完成: attempted={}, succeeded={}, failed={}",
        summary.attempted,
        summary.succeeded,
        summary.failed.len()
    );
    Ok(())
}

async fn refresh_relay_outcome(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
    name: String,
    refresh_config: bool,
) -> RelayRefreshOutcome {
    let provision = if refresh_config {
        Some(refresh_relay_provision(app_handle, relay_id).await)
    } else {
        None
    };
    let balance = relay_balance_impl(app_handle, relay_id).await;
    RelayRefreshOutcome {
        name,
        row_id: relay_id,
        provision,
        balance,
        failures: Vec::new(),
    }
}

pub(crate) async fn refresh_relay_result(
    app_handle: &tauri::AppHandle,
    relay_id: i64,
    app_type: &AppType,
) -> RefreshResult {
    let name = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, |conn| creds::get(conn, relay_id))
            .ok()
            .flatten()
            .map(|relay| {
                if relay.account_label.is_empty() {
                    relay.site_name
                } else {
                    relay.account_label
                }
            })
            .unwrap_or_else(|| format!("中转站 #{relay_id}"))
    };
    finish_refresh_result(
        app_type,
        vec![refresh_relay_outcome(app_handle, relay_id, name, true).await],
        Vec::new(),
    )
}

#[tauri::command]
pub async fn relay_refresh(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    app: String,
) -> Result<RefreshResult, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    Ok(refresh_relay_result(&app_handle, relay_id, &app_type).await)
}

#[tauri::command]
pub async fn relay_refresh_all(
    app_handle: tauri::AppHandle,
    app: String,
) -> Result<RefreshResult, String> {
    let app_type = AppType::from_str(&app).map_err(|error| error.to_string())?;
    let (relay_targets, vendor_targets) = {
        let state = app_handle.state::<AppState>();
        (
            relay_refresh_targets(state.inner(), &app_type).map_err(|error| error.to_string())?,
            crate::commands::vendor::vendor_refresh_targets(state.inner(), &app_type)
                .map_err(|error| error.to_string())?,
        )
    };

    let relay_futures = relay_targets
        .into_iter()
        .map(|(row_id, name, refresh_config)| {
            let app_handle = app_handle.clone();
            async move { refresh_relay_outcome(&app_handle, row_id, name, refresh_config).await }
        });
    let vendor_futures = vendor_targets
        .into_iter()
        .map(|(row_id, name, refresh_config)| {
            let app_handle = app_handle.clone();
            async move {
                let state = app_handle.state::<AppState>();
                let (provision, balance) = crate::commands::vendor::refresh_vendor_account(
                    state.inner(),
                    row_id,
                    refresh_config,
                )
                .await;
                VendorRefreshOutcome {
                    name,
                    row_id,
                    provision,
                    balance,
                }
            }
        });
    let (relay_outcomes, vendor_outcomes) =
        tokio::join!(join_all(relay_futures), join_all(vendor_futures));

    Ok(finish_refresh_result(
        &app_type,
        relay_outcomes,
        vendor_outcomes,
    ))
}

fn collect_refreshed_balance(
    balances: &mut Vec<RefreshedBalance>,
    failures: &mut Vec<RefreshFailure>,
    name: String,
    kind: RefreshedBalanceKind,
    row_id: i64,
    result: Result<balance::RowBalanceResult, AppError>,
) {
    match result {
        Ok(result) => {
            if !result.usage.success {
                failures.push(RefreshFailure {
                    name,
                    reason: result
                        .usage
                        .error
                        .clone()
                        .unwrap_or_else(|| "余额刷新失败".to_string()),
                    kind: None,
                    help_url: None,
                });
            }
            balances.push(RefreshedBalance {
                kind,
                row_id,
                result,
            });
        }
        Err(error) => failures.push(RefreshFailure {
            name,
            reason: error.to_string(),
            kind: None,
            help_url: None,
        }),
    }
}

/// 读当前状态。
///
/// **只读本地**，不发网络请求 —— 这是首屏渲染要等的东西，不该卡在网络上。
/// 「凭据是不是真的还活着」由 [`relay_check_session`] 单独探，前端拿到本地状态先渲染，
/// 再让探活的结果去修正它。
#[tauri::command]
pub fn relay_status(state: State<'_, AppState>) -> Result<RelayStatus, String> {
    relay_status_impl(state.inner()).map_err(|e| e.to_string())
}

/// 探一遍**每一行**已登录的凭据是不是真的还能用，并清掉确认失效的那些**会话**。
///
/// 为什么需要这个：行 DTO 的 `logged_in` 只看本地记的过期时间。而凭据可能在网页端被
/// 撤销、账号被禁用、会话被踢掉 —— 那些情况下本地看起来一切正常，用户点任何操作才会
/// 撞到错误。第 2 次打开 app 到第 100 次都走这条路，不能共用第 1 次的假设。
///
/// ## 为什么是逐行而不是「探当前站」（2026-08-04 改）
///
/// 原来它探的是 `creds::load()` 那一行（全局 `is_current = 1`），返回一个 bool。
/// 那个形状只对「同时只有一个站」的旧界面成立 —— 中转站区是**多行并列**的，
/// 探一行的活等于让另外 N-1 行继续显示错的状态，而用户看不出区别。
///
/// 现在返回**这次被清掉会话的行 id**（空 = 全都还好）。前端据此提示并刷新。
///
/// ⚠️ **清的是会话，不是这一行的全部凭据**：分组与 sk 不受影响，用户点一次
/// 「重新登录」就复原（见 `creds::clear_session`）。
///
/// 未登录的行直接跳过：`usable_relay` 对它们必然 Err，白打一次请求还得过滤噪音。
#[tauri::command]
pub async fn relay_check_session(app_handle: tauri::AppHandle) -> Result<Vec<i64>, String> {
    check_session(&app_handle).await.map_err(|e| e.to_string())
}

async fn check_session(app_handle: &tauri::AppHandle) -> Result<Vec<i64>, AppError> {
    let targets: Vec<i64> = {
        let state = app_handle.state::<AppState>();
        with_conn(&state, creds::list)?
            .into_iter()
            .filter(|site_account| !site_account.auth_token.is_empty())
            .map(|site_account| site_account.id)
            .collect()
    };

    let mut expired = Vec::new();
    // **串行而不是 join_all**：这些请求打的往往是同一个中转站（同一个 IP 段、
    // 同一份 rate limit），而这是启动时的后台探活、没人在等它返回。
    // 并发省下的几百毫秒换来的是撞限流的风险，不值得。
    for id in targets {
        // usable_relay 会在快过期时先续期、并顺手补齐缺失的账号身份（见它的文档）；
        // 拿 /user/profile 当探活请求（最便宜的鉴权端点）。撞上「登录已过期」类 401
        // 时先静默续期再重试一次 —— 那是 `token_expires_at = NULL` 的降级态行唯一
        // 的自救机会（详见 relay_read_with_refresh_retry 的文档）。
        let probe = relay_read_with_refresh_retry(app_handle, id, |site_account| async move {
            backend::RuntimeBackend::for_relay(&site_account)
                .balance()
                .await
        })
        .await;

        if let Err(e) = probe {
            // 「登录态已失效」是 api 层对不可恢复的那一类 401 的措辞（账号被禁 /
            // 会话被撤销 / 用户不存在）。这类清掉本地**会话**、让用户重新登录。
            //
            // ⚠️ **只清会话，不清账号身份**（`clear_session` 而不是 `clear_credentials`）——
            // 分组与 sk 写在各自的 provider 配置里，压根没失效；把 `account_id` 一起
            // 抹掉会让档位按归属过滤时被判成「不是这一行的」⇒ 整片从界面消失，
            // 用户以为密钥没了。完整的三连后果见 `creds::clear_session` 的文档。
            //
            // 其它失败（网络不通、中转站关了用户面板返 403）**连会话都不清** ——
            // 那不是凭据的问题，清掉只会逼用户在网络恢复后白重登一次。
            if should_clear_credentials_after_probe_error(&e) {
                let state = app_handle.state::<AppState>();
                with_conn(&state, |conn| creds::clear_session(conn, id))?;
                let msg = e.to_string();
                log::info!("中转站 {id} 登录态已失效，已清除会话（分组与密钥保留）：{msg}");
                expired.push(id);
            } else {
                let msg = e.to_string();
                log::warn!("中转站 {id} 探活失败但保留凭据（可能只是网络问题）：{msg}");
            }
        }
    }
    Ok(expired)
}

/// 本机还没有任何中转站 / 厂商账号 —— 即「新用户」这一事实的唯一判据。
///
/// `RelayStatus.should_prompt_add_site` 与新人引导（`commands::onboarding`）都读它：
/// 同一个业务事实只算一次，两处不会因为各写一份判据而分叉。
pub(crate) fn user_has_no_accounts(state: &AppState) -> Result<bool, AppError> {
    with_conn(state, |conn| {
        Ok(creds::list(conn)?.is_empty() && crate::vendor::creds::list(conn)?.is_empty())
    })
}

pub(crate) fn relay_status_impl(state: &AppState) -> Result<RelayStatus, AppError> {
    let should_prompt_add_site = user_has_no_accounts(state)?;
    Ok(RelayStatus {
        default_site: DEFAULT_SITE.to_string(),
        should_prompt_add_site,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relay::test_support::*;

    #[test]
    fn relay_status_owns_the_global_add_site_prompt_decision() {
        let db = std::sync::Arc::new(crate::database::Database::memory().expect("init db"));
        let state = AppState::new(db);

        assert!(
            relay_status_impl(&state)
                .expect("empty status")
                .should_prompt_add_site
        );

        with_conn(&state, |conn| {
            crate::vendor::creds::save_account(
                conn,
                crate::vendor::Vendor::DeepSeek,
                "token",
                &crate::vendor::VendorAccount {
                    account_id: "account-7".into(),
                    label: "DeepSeek user".into(),
                    login_identifier: "13800000000".into(),
                },
            )?;
            Ok(())
        })
        .expect("save vendor account");

        assert!(
            !relay_status_impl(&state)
                .expect("configured status")
                .should_prompt_add_site
        );
    }

    #[test]
    fn refresh_summary_counts_vendor_config_for_the_current_app() {
        let summary = refresh_summary(
            &AppType::Codex,
            Vec::new(),
            vec![(
                "DeepSeek".into(),
                Ok(crate::commands::vendor::VendorProvisionSummary {
                    provider_id: "managed-deepseek".into(),
                    platforms: vec![
                        AppType::Codex.as_str().into(),
                        AppType::Claude.as_str().into(),
                    ],
                    key_created: true,
                    merged_providers: vec!["Imported DeepSeek".into()],
                }),
            )],
        );

        assert_eq!(summary.refreshed_accounts, 1);
        assert_eq!(summary.tiers, 1);
        assert_eq!(summary.other_platform_tiers, 0);
        assert_eq!(summary.keys_created, 1);
        assert_eq!(summary.merged_providers, 1);
        assert!(matches!(summary.notice, RefreshNotice::UpdatedWithKeys));
    }

    #[test]
    fn refresh_result_is_cloneable() {
        let result = RefreshResult {
            summary: RefreshSummary {
                notice: RefreshNotice::None,
                refreshed_accounts: 0,
                tiers: 0,
                keys_created: 0,
                other_platform_tiers: 0,
                merged_providers: 0,
                failures: Vec::new(),
            },
            balances: Vec::new(),
        };

        let cloned = result.clone();
        assert!(matches!(cloned.summary.notice, RefreshNotice::None));
    }

    /// 续期救不回来时（refresh token 也被服务端拒了），必须把**原错误**交回去：
    /// `check_session` 靠错误分类决定清会话，换成续期那条报错会悄悄改变判读。
    #[tokio::test]
    async fn relay_read_returns_original_error_when_refresh_cannot_rescue() {
        use axum::{
            http::StatusCode,
            routing::{get, post},
            Json, Router,
        };
        let router = Router::new()
            .route(
                "/api/v1/user/profile",
                get(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "code": "TOKEN_EXPIRED",
                            "message": "登录已过期，请重新登录"
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/auth/refresh",
                post(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "code": "REFRESH_TOKEN_INVALID",
                            "message": "refresh token 已失效"
                        })),
                    )
                }),
            );
        let (origin, server) = spawn_balance_server(router).await;
        let (app, relay_id) = saved_relay_app(&origin, discovery::BackendKind::Sub2Api);

        let error =
            relay_read_with_refresh_retry(app.handle(), relay_id, |site_account| async move {
                backend::RuntimeBackend::for_relay(&site_account)
                    .balance()
                    .await
            })
            .await
            .expect_err("续期失败时原请求的错误必须往外抛");

        assert!(error.to_string().contains("登录已过期"), "{error}");
        assert!(!error.to_string().contains("续期失败"), "{error}");
        let creds = relay_credentials(&app, relay_id);
        assert_eq!(
            creds.auth_token, "saved-access-token",
            "续期失败不得改动库里的凭据"
        );
        server.abort();
    }

    #[tokio::test]
    async fn pricing_refresh_skips_fresh_rows_and_continues_after_one_failure() {
        let mut fresh = test_newapi_relay(1);
        fresh.pricing_synced_at = Some(100);
        let failed = test_newapi_relay(2);
        let succeeded = test_newapi_relay(3);
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_refresh = Arc::clone(&attempts);

        let summary = refresh_due_relay_pricing_rows(
            vec![fresh, failed, succeeded],
            159,
            std::time::Duration::from_secs(60),
            move |relay| {
                let attempts = Arc::clone(&attempts_for_refresh);
                async move {
                    attempts.lock().unwrap().push(relay.id);
                    if relay.id == 2 {
                        Err(AppError::Message("expected failure".into()))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert_eq!(summary.attempted, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].0, 2);
        let mut attempted_ids = attempts.lock().unwrap().clone();
        attempted_ids.sort_unstable();
        assert_eq!(attempted_ids, vec![2, 3]);
    }
}
