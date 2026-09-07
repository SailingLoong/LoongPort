//! 一行余额的**有序回落链**：cc-switch → sub2api sk → 网页登录态。
//!
//! ## 为什么要一条链，而不是各自一条路
//!
//! 原来「中转站行」与「官网行」各有一条**只走网页登录态**的余额路：中转站走 JWT 打
//! `/api/v1/user/profile`，官网走厂商的网页会话。两条路的共同前提是「登录态还活着」，
//! 而 sk 是**独立凭据** —— 登录态过期时 sk 照样能调用，用户却看不到余额，连充值入口
//! 都跟着消失（充值按钮只在有余额时渲染）。
//!
//! 而 cc-switch 本来就有一套「用 sk 查余额 → [`UsageResult`]」的实现
//! （[`crate::services::balance::get_balance`]，认 DeepSeek / StepFun / SiliconFlow /
//! OpenRouter / Novita），fork 却没用上它。
//!
//! ## 顺序是维护者定的，且顺序本身有语义
//!
//! | 步 | 路 | 命中谁 |
//! |---|---|---|
//! | 1 | [`crate::services::balance::get_balance`]（上游，按 base_url 主机名认厂商） | 官网行（DeepSeek 等） |
//! | 2 | [`sub2api::usage_with_api_key`]（sub2api 的 `GET /v1/usage`，**sk 鉴权**） | sub2api 中转站行 |
//! | 3 | 网页登录态（见 [`SessionFallback`]） | NewAPI 中转站行 —— **它只有这一条** |
//!
//! ⚠️ **顺序写反不会报错**，只会让每一行白打一轮无用请求：
//! - 中转站的站点域名 cc-switch **认不出** ⇒ 第 1 步对它是**零请求的空转**
//!   （`detect_provider` 返回 `None` 时直接 `Ok(success:false)`，不发任何请求），
//!   所以把它放在最前面不花代价。
//! - 反过来，官网行若先走第 2 步，就是朝 `api.deepseek.com/v1/usage` 打一个必定
//!   404 的请求。
//!
//! ⚠️ **第 3 步不能删**：NewAPI 中转站没有 sk 鉴权的 `/v1/usage`，JWT 是它唯一的路。
//!
//! ## 三步都拿不到 ⇒ `success:false`，**不是 `Err`**
//!
//! 这条决定前端是「渲染失败态 + 重试入口」还是「整块静默消失」。改造前的死路正是
//! 后者：余额由一个依赖键为 `id:accountLabel` 的 effect 拉，某一行失败过一次、键不变
//! ⇒ effect 永不重跑 ⇒ 那一行整个会话都没有余额，而充值按钮又只在有余额时存在 ⇒
//! 用户连重试的入口都看不到。返回 `success:false` 让 react-query 拿到一个**可显示的
//! 失败值**，用量条渲染失败态并保留刷新按钮。
//!
//! 语义与 [`crate::services::balance`] 那份完全一致（`Err` 只留给瞬时传输失败），
//! 所以两边的结果能进同一个前端组件。

use futures::future::join_all;

use crate::error::AppError;
use crate::provider::{UsageData, UsageResult};
use crate::relay::{backend, creds, sub2api};

const LOW_BALANCE_THRESHOLD_USD: f64 = 5.0;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowBalanceResult {
    pub usage: UsageResult,
    pub should_prompt_top_up: bool,
}

pub fn row_balance_result(usage: UsageResult, top_up_prompt_applicable: bool) -> RowBalanceResult {
    let should_prompt_top_up = top_up_prompt_applicable
        && usage.success
        && usage
            .data
            .as_ref()
            .and_then(|items| items.first())
            .and_then(|item| item.remaining)
            .is_some_and(|remaining| remaining < LOW_BALANCE_THRESHOLD_USD);

    RowBalanceResult {
        usage,
        should_prompt_top_up,
    }
}

/// 一行的查询材料。
///
/// **打成结构体而不是三个平铺参数**：`site_origin` 与 `base_url` 都是 URL 形状的
/// `&str`，调换了编译器不会报，而后果是第 1 步认错厂商、第 2 步打错站点。
#[derive(Debug, Clone, Copy)]
pub struct BalanceQuery<'a> {
    /// 站点根（形如 `https://example.com`），第 2 步拿它拼 `/v1/usage`。
    pub site_origin: &'a str,
    /// 第 1 步用来**认厂商**的 base_url（[`crate::services::balance`] 按主机名判）。
    /// 官网行给厂商的 API 根（`https://api.deepseek.com`），中转站行给站点算出的那个。
    pub base_url: &'a str,
    /// 这一行名下的 sk。第 1、2 步各自并发试完，第一把问出结果的胜出。
    pub api_keys: &'a [String],
}

/// 第 3 步用哪条**网页登录态**路。
///
/// 两类行在这一步天生不同：中转站走 sub2api 的 JWT `/user/profile`，官网走 DeepSeek
/// 自己的 `/api/v0/users/get_user_summary`。前两步（sk）两边完全一样，所以差异**只在
/// 这一个 enum 上** —— 顺序本身仍然只在本模块定义一次，调用方无从改动它。
pub enum SessionFallback<'a> {
    /// sub2api / NewAPI 中转站的 JWT 路。**NewAPI 站只有这一条**，不能删。
    Relay(&'a creds::RelayAccount),
    /// 官网厂商的网页登录态路。每家的会话接口不同（`vendor::balance` 分发）。
    Vendor {
        vendor: crate::vendor::Vendor,
        auth_token: &'a str,
    },
    /// 这一行没有可用登录态 ⇒ 跳过第 3 步，别去打一个必定 401 的请求。
    None,
}

/// 链上的一步。**做成 enum + [`BalanceStep::next`] 而不是三条顺序语句**：顺序是这个
/// 模块的核心契约，让它成为一个能被测试直接断言的值，而不是散在控制流里的行序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalanceStep {
    CcSwitch,
    Sub2Api,
    Session,
}

impl BalanceStep {
    fn next(self) -> Option<Self> {
        match self {
            Self::CcSwitch => Some(Self::Sub2Api),
            Self::Sub2Api => Some(Self::Session),
            Self::Session => None,
        }
    }
}

/// [`resolve`] 的产物。
pub struct Resolved {
    /// 给前端的余额结果。**永远是 `Ok` 语义**，失败体现为 `success:false`。
    pub usage: UsageResult,
    /// 官网那条路遇到的结构化错误（只有第 3 步的 [`SessionFallback::Vendor`] 会填）。
    /// 命令层靠它判 [`crate::vendor::VendorError::AuthExpired`] 要不要清 token ——
    /// 本仓规矩是不许靠字符串匹配分派，所以它顺着这里带出去，命令层不必为了拿这一个
    /// 判断再打一次同样的请求。
    pub vendor_error: Option<crate::vendor::VendorError>,
}

/// 按固定顺序查询余额，第一个成功的结果胜出。
///
/// **永不返回 `Err`**：三条路都失败时回 `success:false` + 拼起来的错因，
/// 见模块文档最后一节。
pub async fn resolve(query: BalanceQuery<'_>, session: SessionFallback<'_>) -> Resolved {
    let mut errors: Vec<String> = Vec::new();
    let mut vendor_error = None;
    let mut step = Some(BalanceStep::CcSwitch);

    while let Some(current) = step {
        let hit = match current {
            BalanceStep::CcSwitch => {
                cc_switch_balance(query.base_url, query.api_keys, &mut errors).await
            }
            BalanceStep::Sub2Api => {
                sub2api_balance(query.site_origin, query.api_keys, &mut errors).await
            }
            BalanceStep::Session => session_balance(&session, &mut errors, &mut vendor_error).await,
        };
        if let Some(usage) = hit {
            return Resolved {
                usage,
                vendor_error,
            };
        }
        step = current.next();
    }

    Resolved {
        usage: UsageResult {
            success: false,
            data: None,
            // 三步的失败原因**全带出去**：只留最后一条会把「sk 全都 401」这类真原因
            // 盖成一句「登录态过期」，而那恰恰指错了要用户做的动作。
            error: Some(if errors.is_empty() {
                "查不到余额：这一行还没有可用的密钥或登录态".to_string()
            } else {
                errors.join("；")
            }),
        },
        vendor_error,
    }
}

/// 第 1 步：cc-switch 的按厂商实现。
///
/// 中转站域名它认不出 ⇒ 每把 sk 都是**零请求**的 `Ok(success:false)`，白跑不花钱。
async fn cc_switch_balance(
    base_url: &str,
    api_keys: &[String],
    errors: &mut Vec<String>,
) -> Option<UsageResult> {
    let results = join_all(
        api_keys
            .iter()
            .map(|api_key| crate::services::balance::get_balance(base_url, api_key)),
    )
    .await;

    for result in results {
        match result {
            Ok(usage) if usage.success => return Some(usage),
            Ok(usage) => errors.extend(usage.error),
            Err(error) => errors.push(error),
        }
    }
    None
}

/// 第 2 步：sub2api 的 sk 鉴权 `/v1/usage`。**只认 `balance`**（见 [`sub2api::usage_with_api_key`]）。
async fn sub2api_balance(
    site_origin: &str,
    api_keys: &[String],
    errors: &mut Vec<String>,
) -> Option<UsageResult> {
    let results = join_all(
        api_keys
            .iter()
            .map(|api_key| sub2api::usage_with_api_key(site_origin, api_key)),
    )
    .await;

    for result in results {
        match result {
            Ok(usage) if usage.success => return Some(usage),
            Ok(usage) => errors.extend(usage.error),
            Err(error) => errors.push(error.to_string()),
        }
    }
    None
}

/// 第 3 步：网页登录态。两类行各有自己的接口，见 [`SessionFallback`]。
async fn session_balance(
    session: &SessionFallback<'_>,
    errors: &mut Vec<String>,
    vendor_error: &mut Option<crate::vendor::VendorError>,
) -> Option<UsageResult> {
    match session {
        SessionFallback::None => None,
        SessionFallback::Relay(relay) => {
            match backend::RuntimeBackend::for_relay(relay).balance().await {
                Ok(balance) => Some(wallet_usage(balance.balance)),
                Err(error) => {
                    errors.push(error.to_string());
                    None
                }
            }
        }
        SessionFallback::Vendor { vendor, auth_token } => {
            match crate::vendor::balance(*vendor, auth_token).await {
                Ok(Some(usage)) => Some(usage),
                // 登录态活着但这个账号没有钱包 ⇒ 确实没有余额可显示，不当成错误，
                // 也不编造一个 0（见 `deepseek::wallet_usage`）。
                Ok(None) => None,
                Err(error) => {
                    errors.push(crate::error::AppError::from(error.clone()).to_string());
                    *vendor_error = Some(error);
                    None
                }
            }
        }
    }
}

/// JWT 路拿到的钱包余额包成 [`UsageResult`]。
///
/// **只填 `remaining`，不填 `total` / `used`** —— 钱包没有「总额」这个概念（充多少是
/// 多少），编造一个 `total` 会让前端的「剩余不足 10%」配色按一个假分母算。
///
/// 单位写 `USD`：sub2api 的钱包就是美元计价（`/v1/usage` 自己回的也是 `"USD"`）。
/// 留空会让同一行在两条路之间切换时数字旁边的单位忽隐忽现。
fn wallet_usage(balance: f64) -> UsageResult {
    UsageResult {
        success: true,
        data: Some(vec![UsageData {
            plan_name: Some("钱包余额".to_string()),
            remaining: Some(balance),
            unit: Some("USD".to_string()),
            extra: None,
            is_valid: None,
            invalid_message: None,
            total: None,
            used: None,
        }]),
        error: None,
    }
}

// ============================================================================
// 站点余额缓存（省心看板 SWR）
// ============================================================================

/// 看板余额的缓存表：`origin → (余额, 拉取时刻)`，每站一行。
///
/// 由 `create_tables_on_conn`（全新库）与 LoongPort 迁移 v18 → v19（老库）
/// 共同调用，两边建的必须是同一形态 —— 见 `database/loongport_schema.rs`
/// 的头注释。
pub fn create_site_balance_cache_table(conn: &rusqlite::Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS site_balance_cache (
            site_origin TEXT PRIMARY KEY,
            balance_usd REAL,
            fetched_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 site_balance_cache 表失败: {e}")))?;
    Ok(())
}

/// 缓存 TTL（秒）：超过视为 stale，看板读到时踢一次后台刷新。
pub const SITE_BALANCE_TTL_SECS: i64 = 600;

/// 单站余额链预算。usage → billing 双端点各自 30s 总超时且串行，不设预算时
/// 一家黑洞站最坏拖 90s —— 刷新虽已在后台，超预算的站仍会长期占着单飞槽位。
const SITE_BALANCE_FETCH_BUDGET_SECS: u64 = 15;

/// 一行缓存：`balance` 为 `None` = 负缓存（最近查过、这家没有可用值）——
/// 没有负缓存的话，查不出余额的站每次打开看板都会重查一遍。
pub type SiteBalanceEntry = (Option<f64>, i64);

/// 读全表（行数 = 站点数，个位到几十）。
pub fn cached_site_balances(
    db: &crate::database::Database,
) -> std::collections::HashMap<String, SiteBalanceEntry> {
    // 非 Result 返回值的锁惯例：毒锁取内值（与 commands::auto_mode 的直查一致），
    // 读缓存失败按「无缓存」处理 —— 看板照常返回，stale 判定自然触发刷新。
    let conn = db
        .conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut stmt =
        match conn.prepare("SELECT site_origin, balance_usd, fetched_at FROM site_balance_cache") {
            Ok(stmt) => stmt,
            Err(e) => {
                log::warn!("读 site_balance_cache 失败（按无缓存处理）: {e}");
                return std::collections::HashMap::new();
            }
        };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, Option<f64>>(1)?, row.get::<_, i64>(2)?),
        ))
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(e) => {
            log::warn!("遍历 site_balance_cache 失败（按无缓存处理）: {e}");
            std::collections::HashMap::new()
        }
    }
}

/// 读单站缓存（行级余额条的读路径）。`None` = 没进过缓存。
pub fn cached_site_balance(
    db: &crate::database::Database,
    origin: &str,
) -> Option<SiteBalanceEntry> {
    let conn = db
        .conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    conn.query_row(
        "SELECT balance_usd, fetched_at FROM site_balance_cache WHERE site_origin = ?1",
        rusqlite::params![origin],
        |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, i64>(1)?)),
    )
    .ok()
}

/// 从缓存条目拼行级展示结果（正/负缓存同一入口）。
///
/// 负缓存（`balance = None`）的失败文案是泛化的 —— 具体错误原因不进缓存
/// （会过期、也占库），前端 keep-last-good 与手动刷新（force 旁路）兜住展示。
pub fn cached_row_balance_result(entry: &SiteBalanceEntry) -> RowBalanceResult {
    let usage = match entry.0 {
        Some(balance) => UsageResult {
            success: true,
            data: Some(vec![UsageData {
                plan_name: None,
                extra: None,
                is_valid: None,
                invalid_message: None,
                total: None,
                used: None,
                remaining: Some(balance),
                unit: Some("USD".to_string()),
            }]),
            error: None,
        },
        None => UsageResult {
            success: false,
            data: None,
            error: Some("最近一次查询没有取到可用余额".to_string()),
        },
    };
    row_balance_result(usage, true)
}

/// 幂等写一行（含负缓存）。
pub fn upsert_site_balance(
    db: &crate::database::Database,
    origin: &str,
    entry: SiteBalanceEntry,
) -> Result<(), AppError> {
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO site_balance_cache (site_origin, balance_usd, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(site_origin) DO UPDATE SET
            balance_usd = excluded.balance_usd,
            fetched_at = excluded.fetched_at",
        rusqlite::params![origin, entry.0, entry.1],
    )
    .map_err(|e| AppError::Database(format!("写 site_balance_cache 失败: {e}")))?;
    Ok(())
}

/// 删指定站的缓存行 —— 充值等「余额已确定变化」的时刻用：旧值必然错了，
/// 留着只会误导，删掉后由紧随的单站刷新重新落值。
pub fn drop_site_cache(
    db: &crate::database::Database,
    origins: &std::collections::HashSet<String>,
) {
    let conn = db
        .conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for origin in origins {
        if let Err(e) = conn.execute(
            "DELETE FROM site_balance_cache WHERE site_origin = ?1",
            rusqlite::params![origin],
        ) {
            log::warn!("[site-balance] 删缓存失败 {origin}: {e}");
        }
    }
}

/// 哪些站需要刷新：没进过缓存的 + `fetched_at` 超 TTL 的（纯函数）。
fn stale_balance_sites(
    wanted: &std::collections::HashMap<String, String>,
    cached: &std::collections::HashMap<String, SiteBalanceEntry>,
    now: i64,
) -> Vec<(String, String)> {
    wanted
        .iter()
        .filter(|(origin, _)| match cached.get(*origin) {
            Some((_, fetched_at)) => now - *fetched_at > SITE_BALANCE_TTL_SECS,
            None => true,
        })
        .map(|(origin, key)| (origin.clone(), key.clone()))
        .collect()
}

/// 单飞集合：正在后台刷新的 origin。看板查询可能高频触发（多 app + 窗口聚焦），
/// 没有这道闸会叠出一串重复扇出。
static REFRESH_INFLIGHT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

/// 读时惰性刷新（SWR 的 revalidate）：筛出 stale 站点交给后台任务 ——
/// sk 直查 → 写缓存 → 发 [`crate::events::SITE_BALANCES_UPDATED`] 让前端补值。
///
/// **这不是 interval 轮询**：唯一触发点是「看板被读」这类用户可见时刻，与
/// 2026-08「余额采样零新增路径、不做后台轮询」的决策（PR #127）一致。那条
/// 决策防的是**登录态路**的周期请求 —— NewAPI 的 refresh cookie 一次性轮换，
/// 充值窗口持有独占权时后台续期会把用户踢出充值页。本链路只走 sk
/// （`usage_with_api_key` / `billing_balance_with_api_key`），不携带登录态、
/// 不碰 cookie。⚠️ 若将来把这条链改走登录态，必须先补「充值窗口活跃站跳过」。
#[cfg(feature = "gui")]
pub fn spawn_stale_refresh<R: tauri::Runtime>(
    db: std::sync::Arc<crate::database::Database>,
    app_handle: Option<tauri::AppHandle<R>>,
    wanted: std::collections::HashMap<String, String>,
) {
    let now = chrono::Utc::now().timestamp();
    let stale = {
        let cached = cached_site_balances(&db);
        let stale = stale_balance_sites(&wanted, &cached, now);
        if stale.is_empty() {
            return;
        }
        let inflight = REFRESH_INFLIGHT.get_or_init(Default::default);
        let Ok(mut guard) = inflight.lock() else {
            return;
        };
        let fresh: Vec<_> = stale
            .into_iter()
            .filter(|(origin, _)| guard.insert(origin.clone()))
            .collect();
        fresh
    };
    if stale.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let origins: Vec<String> = stale.iter().map(|(origin, _)| origin.clone()).collect();
        let results = futures::future::join_all(stale.into_iter().map(|(origin, key)| async move {
            let fetched = match tokio::time::timeout(
                std::time::Duration::from_secs(SITE_BALANCE_FETCH_BUDGET_SECS),
                fetch_site_balance(&origin, &key),
            )
            .await
            {
                Ok(balance) => balance,
                Err(_elapsed) => {
                    log::warn!("[site-balance] {origin} 余额链超预算（{SITE_BALANCE_FETCH_BUDGET_SECS}s），本次记负缓存");
                    None
                }
            };
            (origin, fetched)
        }))
        .await;

        let now = chrono::Utc::now().timestamp();
        for (origin, balance) in results {
            if let Err(e) = upsert_site_balance(&db, &origin, (balance, now)) {
                log::warn!("[site-balance] 写缓存失败 {origin}: {e}");
            }
        }

        if let Some(inflight) = REFRESH_INFLIGHT.get() {
            if let Ok(mut guard) = inflight.lock() {
                for origin in origins {
                    guard.remove(&origin);
                }
            }
        }

        if let Some(app_handle) = app_handle {
            crate::events::emit_site_balances_updated(&app_handle);
        }
    });
}

/// 单站余额链（sk 直查，从看板原 `fetch_site_balances` 迁入）：
/// sub2api `/v1/usage` 钱包 → one-api 系 billing 双端点回落。
async fn fetch_site_balance(origin: &str, key: &str) -> Option<f64> {
    let sub2api_wallet = crate::relay::sub2api::usage_with_api_key(origin, key)
        .await
        .ok()
        .and_then(|usage| {
            usage
                .data
                .and_then(|items| items.first().and_then(|item| item.remaining))
        });
    match sub2api_wallet {
        Some(balance) => Some(balance),
        None => crate::relay::sub2api::billing_balance_with_api_key(origin, key)
            .await
            .ok()
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== 站点余额缓存（看板 SWR） ====================

    fn cache_db() -> crate::database::Database {
        // Database::memory() 已按生产 schema 建齐全部表（含 v19 的
        // site_balance_cache），别自建。
        crate::database::Database::memory().expect("内存库")
    }

    /// 缓存读写 roundtrip：正/负缓存（None）都要能原样读回、upsert 幂等覆盖。
    #[test]
    fn site_balance_cache_roundtrips_including_negative_entries() {
        let db = cache_db();
        let now = 1_000_000_i64;
        upsert_site_balance(&db, "https://a.example", (Some(9.5), now)).unwrap();
        upsert_site_balance(&db, "https://dead.example", (None, now)).unwrap();

        let cached = cached_site_balances(&db);
        assert_eq!(cached.get("https://a.example"), Some(&(Some(9.5), now)));
        assert_eq!(
            cached.get("https://dead.example"),
            Some(&(None, now)),
            "负缓存（查过无值）也要占位，否则死站每次打开都重查"
        );
        assert!(!cached.contains_key("https://never.example"));

        // 覆盖写：同站新值顶旧值
        upsert_site_balance(&db, "https://a.example", (Some(8.0), now + 1)).unwrap();
        assert_eq!(
            cached_site_balances(&db).get("https://a.example"),
            Some(&(Some(8.0), now + 1))
        );
    }

    /// stale 判定：没进过缓存的、超 TTL 的要刷；TTL 内的（含负缓存）不刷。
    #[test]
    fn stale_balance_sites_pick_missing_and_over_ttl_only() {
        let now = 10_000_i64;
        let fresh = now - (SITE_BALANCE_TTL_SECS - 1);
        let stale = now - (SITE_BALANCE_TTL_SECS + 1);
        let mut wanted = std::collections::HashMap::new();
        wanted.insert("https://fresh.example".to_string(), "k1".to_string());
        wanted.insert("https://stale.example".to_string(), "k2".to_string());
        wanted.insert("https://missing.example".to_string(), "k3".to_string());
        wanted.insert(
            "https://fresh-negative.example".to_string(),
            "k4".to_string(),
        );
        let mut cached = std::collections::HashMap::new();
        cached.insert("https://fresh.example".to_string(), (Some(1.0), fresh));
        cached.insert("https://stale.example".to_string(), (Some(2.0), stale));
        cached.insert("https://fresh-negative.example".to_string(), (None, fresh));

        let mut stale_sites = stale_balance_sites(&wanted, &cached, now);
        stale_sites.sort();
        assert_eq!(
            stale_sites,
            vec![
                ("https://missing.example".to_string(), "k3".to_string()),
                ("https://stale.example".to_string(), "k2".to_string()),
            ],
            "TTL 内的正/负缓存都不刷；缺缓存与超 TTL 的要刷"
        );
    }

    /// 后台管线贯通：不可达站点跑完「fetch（快速失败）→ 写缓存」后留下负缓存，
    /// 下一次看板读取（TTL 内）不再重查。
    #[tokio::test]
    async fn background_refresh_writes_negative_cache_for_unreachable_site() {
        let db = std::sync::Arc::new(cache_db());
        let mut wanted = std::collections::HashMap::new();
        // .example 是保留 TLD，DNS 必然快速失败（离线环境同样快速失败）
        wanted.insert(
            "https://nonexistent.example".to_string(),
            "sk-x".to_string(),
        );

        crate::relay::balance::spawn_stale_refresh(db.clone(), None::<tauri::AppHandle>, wanted);

        for _ in 0..250 {
            let cached = cached_site_balances(&db);
            if let Some((balance, _)) = cached.get("https://nonexistent.example") {
                assert_eq!(*balance, None, "不可达站必须是负缓存而不是有值");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("后台刷新没有为不可达站写下负缓存");
    }

    #[test]
    fn relay_wallet_balance_owns_the_top_up_prompt_fact() {
        let low = row_balance_result(wallet_usage(4.99), true);
        let threshold = row_balance_result(wallet_usage(5.0), true);
        let vendor = row_balance_result(wallet_usage(1.0), false);

        assert!(low.should_prompt_top_up);
        assert!(!threshold.should_prompt_top_up);
        assert!(!vendor.should_prompt_top_up);
    }

    /// 走到 `success_at` 那一步为止，链上依次经过了哪些步。
    fn steps_until_success(success_at: BalanceStep) -> Vec<BalanceStep> {
        let mut visited = Vec::new();
        let mut step = Some(BalanceStep::CcSwitch);
        while let Some(current) = step {
            visited.push(current);
            if current == success_at {
                break;
            }
            step = current.next();
        }
        visited
    }

    /// ⭐ **顺序是这个模块的全部价值，而写反了不会报任何错。**
    ///
    /// 反过来的两种后果都只是「悄悄变慢/变错」：中转站行会先朝一个认不出它的厂商表
    /// 白问一轮，官网行会先朝 `api.deepseek.com/v1/usage` 打一个必定 404 的请求。
    #[test]
    fn balance_fallback_order_is_fixed() {
        assert_eq!(
            steps_until_success(BalanceStep::CcSwitch),
            vec![BalanceStep::CcSwitch],
            "cc-switch 命中就该收工，不该继续问 sub2api"
        );
        assert_eq!(
            steps_until_success(BalanceStep::Sub2Api),
            vec![BalanceStep::CcSwitch, BalanceStep::Sub2Api],
            "sub2api 必须排在 cc-switch 之后"
        );
        assert_eq!(
            steps_until_success(BalanceStep::Session),
            vec![
                BalanceStep::CcSwitch,
                BalanceStep::Sub2Api,
                BalanceStep::Session
            ],
            "网页登录态是最后一条 —— 它是 NewAPI 唯一的路，但前两条不需要登录态"
        );
    }

    /// ⭐ 三步都拿不到时返回 `success:false` 而**不是 `Err`**。
    ///
    /// 这条决定前端是渲染「失败态 + 刷新按钮」还是让整块余额区消失（那样用户连重查
    /// 的入口都没有 —— 正是改造前那个死路）。
    #[tokio::test]
    async fn all_steps_failing_returns_unsuccessful_usage_result() {
        let resolved = resolve(
            BalanceQuery {
                site_origin: "https://relay.example",
                base_url: "",
                api_keys: &[],
            },
            SessionFallback::None,
        )
        .await;

        assert!(!resolved.usage.success);
        assert!(resolved.usage.data.is_none());
        assert!(
            resolved.usage.error.is_some(),
            "失败必须带原因，否则前端只能显示一个空白的失败态"
        );
    }

    /// ⭐ **订阅型分组的额度不是钱包余额。**
    ///
    /// 那种响应有 `remaining`、没有 `balance`。认 `remaining` 会把「这个分组今天还剩
    /// 多少额度」显示成「账户里还有多少钱」—— 数字看着像真的，含义完全不同。
    #[test]
    fn subscription_usage_without_balance_is_not_wallet_balance() {
        let result = sub2api::parse_usage_with_api_key_response(
            r#"{"mode":"unrestricted","isValid":true,"planName":"月付组","remaining":42.0,"unit":"USD"}"#,
        )
        .expect("订阅型响应仍是合法 JSON");

        assert!(!result.success, "没有 balance 就是「没问出钱包余额」");
        assert!(result.data.is_none(), "不能把订阅额度当成余额透出去");
    }

    #[test]
    fn wallet_usage_preserves_balance_plan_name_and_unit() {
        let result = sub2api::parse_usage_with_api_key_response(
            r#"{"mode":"unrestricted","isValid":true,"planName":"钱包余额","remaining":12.5,"unit":"USD","balance":12.5}"#,
        )
        .expect("钱包型响应应能解析");

        assert!(result.success);
        let usage = result
            .data
            .and_then(|data| data.into_iter().next())
            .expect("应返回一条钱包余额");
        assert_eq!(usage.plan_name.as_deref(), Some("钱包余额"));
        assert_eq!(usage.remaining, Some(12.5));
        assert_eq!(usage.unit.as_deref(), Some("USD"));
    }

    /// ⭐ newapi（one-api 家族）sk 直查余额：billing 双端点合成，
    /// 余额 = `hard_limit_usd` − `total_usage`（美分）/100。
    #[test]
    fn billing_subscription_and_usage_compose_wallet_balance() {
        let balance = sub2api::parse_billing_balance(
            r#"{"object":"billing.subscription","has_payment_method":true,"hard_limit_usd":12.5}"#,
            r#"{"object":"list","total_usage":250.0}"#,
        )
        .expect("双端点合法 JSON 应能合成余额");
        assert_eq!(balance, Some(10.0), "12.5 − 250美分/100 = 10.0 美元");
    }

    /// ⭐ 缺任一字段/端点不可用（空响应）不算数，坏 JSON 是 Err。
    /// 只有额度没有用量时宁可不显示，也不能把额度当余额。
    #[test]
    fn billing_missing_fields_or_empty_bodies_are_not_a_balance() {
        assert_eq!(
            sub2api::parse_billing_balance(
                r#"{"object":"billing.subscription"}"#,
                r#"{"object":"list","total_usage":1.0}"#,
            )
            .unwrap(),
            None,
            "没有 hard_limit_usd 就不猜"
        );
        assert_eq!(
            sub2api::parse_billing_balance(
                r#"{"object":"billing.subscription","hard_limit_usd":5.0}"#,
                r#"{"object":"list"}"#,
            )
            .unwrap(),
            None,
            "没有 total_usage 就不猜"
        );
        // 端点不可用（404/403/401）时上层给空串 —— 查不到不算错
        assert_eq!(sub2api::parse_billing_balance("", r#"{}"#).unwrap(), None);
        assert_eq!(
            sub2api::parse_billing_balance(r#"{"hard_limit_usd":1.0}"#, "").unwrap(),
            None
        );
        assert!(sub2api::parse_billing_balance("not json", r#"{}"#).is_err());
        assert!(sub2api::parse_billing_balance(r#"{}"#, "not json").is_err());
    }

    /// ⭐ new-api「无限额度」哨兵不是余额：实测站点三个 limit 全返 1e8、usage 为 0
    /// ——把一亿美元当余额显示是笑话。超出常规预充值量级（$1e6）即按查不到处理。
    #[test]
    fn billing_unlimited_sentinel_is_not_a_balance() {
        let balance = sub2api::parse_billing_balance(
            r#"{"object":"billing_subscription","has_payment_method":true,"soft_limit_usd":100000000,"hard_limit_usd":100000000,"system_hard_limit_usd":100000000,"access_until":0}"#,
            r#"{"object":"list","total_usage":0}"#,
        )
        .unwrap();
        assert_eq!(balance, None, "哨兵量级的余额必须被拒收");
    }
}
