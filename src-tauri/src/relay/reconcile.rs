//! `relay_balance_snapshots` 表：中转站余额快照（扣费对账的唯一采样点）。
//!
//! ## 快照从哪来
//!
//! 写入挂在 `relay_balance_impl` 成功解析余额之后（前端行级刷新、充值关窗都会触发），
//! 本模块只负责表 + DAO；对账窗口计算是后续任务的事。
//!
//! ## `created_at` 的单位：Unix 秒
//!
//! 与 `proxy_request_logs.created_at` 同单位 —— 那边写的是
//! `chrono::Utc::now().timestamp()`（秒，见 `proxy/usage/logger.rs`）。
//! 对账要把两种时间放在同一个窗口里比，单位必须一致。

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::database::lock_conn;
use crate::error::AppError;
use crate::provider::UsageResult;
use crate::Database;

/// 一行余额快照。
#[derive(Debug, Clone, PartialEq)]
pub struct BalanceSnapshot {
    pub id: i64,
    pub relay_id: i64,
    pub balance_usd: f64,
    /// 采样来源（如 `balance_query`）。
    pub source: String,
    /// Unix 秒，与 `proxy_request_logs.created_at` 同单位。
    pub created_at: i64,
}

/// 建表 + 索引（幂等）。
///
/// 由 `create_tables_on_conn`（全新库）与 LoongPort 迁移 v13 → v14（老库）共同调用，
/// 两边建的必须是同一形态 —— 见 `database/loongport_schema.rs` 的头注释。
pub fn create_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS relay_balance_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            relay_id INTEGER NOT NULL,
            balance_usd REAL NOT NULL,
            source TEXT NOT NULL DEFAULT 'balance_query',
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 relay_balance_snapshots 表失败: {e}")))?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_relay_balance_snapshots_relay_time
         ON relay_balance_snapshots (relay_id, created_at)",
        [],
    )
    .map_err(|e| AppError::Database(format!("创建 relay_balance_snapshots 索引失败: {e}")))?;

    Ok(())
}

impl Database {
    /// 记一条余额快照，返回行 id。`created_at` 取当前 Unix 秒。
    pub fn insert_balance_snapshot(
        &self,
        relay_id: i64,
        balance_usd: f64,
        source: &str,
    ) -> Result<i64, AppError> {
        let conn = lock_conn!(self.conn);
        let created_at = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO relay_balance_snapshots (relay_id, balance_usd, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![relay_id, balance_usd, source, created_at],
        )
        .map_err(|e| AppError::Database(format!("写入余额快照失败: {e}")))?;
        Ok(conn.last_insert_rowid())
    }

    /// 列出某个中转站的快照，按时间升序（对账窗口要按先后配对）。
    ///
    /// `since_secs` 给了就只取该时刻（含）之后的，单位与 `created_at` 一致（Unix 秒）。
    pub fn list_balance_snapshots(
        &self,
        relay_id: i64,
        since_secs: Option<i64>,
    ) -> Result<Vec<BalanceSnapshot>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = match since_secs {
            Some(_) => conn
                .prepare(
                    "SELECT id, relay_id, balance_usd, source, created_at
                     FROM relay_balance_snapshots
                     WHERE relay_id = ?1 AND created_at >= ?2
                     ORDER BY created_at, id",
                )
                .map_err(|e| AppError::Database(format!("准备查询余额快照失败: {e}")))?,
            None => conn
                .prepare(
                    "SELECT id, relay_id, balance_usd, source, created_at
                     FROM relay_balance_snapshots
                     WHERE relay_id = ?1
                     ORDER BY created_at, id",
                )
                .map_err(|e| AppError::Database(format!("准备查询余额快照失败: {e}")))?,
        };

        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(BalanceSnapshot {
                id: row.get(0)?,
                relay_id: row.get(1)?,
                balance_usd: row.get(2)?,
                source: row.get(3)?,
                created_at: row.get(4)?,
            })
        };

        let rows = match since_secs {
            Some(since) => stmt
                .query_map(params![relay_id, since], map_row)
                .map_err(|e| AppError::Database(format!("查询余额快照失败: {e}")))?,
            None => stmt
                .query_map(params![relay_id], map_row)
                .map_err(|e| AppError::Database(format!("查询余额快照失败: {e}")))?,
        };

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Database(format!("读取余额快照失败: {e}")))?);
        }
        Ok(out)
    }
}

/// `relay_balance_impl` 成功解析余额之后的旁路采样：满足写入条件就落一条快照。
///
/// 写入条件（plan §三.2）：`usage.success` 且**第一条**数据有 `remaining`、`unit` 是
/// `"USD"`（与 `row_balance_result` 取同一条 —— 那边判低余额告警也是取第一条）。
/// 失败路径不写：`success:false` 是常态（如 sub2api 订阅型分组 sk 路查不到余额），
/// 天然无快照，对账页显示「快照不足」即可。
///
/// **写快照失败不影响余额显示**：对账是旁路能力，这里只记日志，绝不向上抛。
pub fn capture_balance_snapshot(db: &Database, relay_id: i64, usage: &UsageResult) {
    let Some(balance_usd) = resolved_usd_balance(usage) else {
        return;
    };
    if let Err(error) = db.insert_balance_snapshot(relay_id, balance_usd, "balance_query") {
        log::warn!("写入余额快照失败（不影响余额显示，对账页会显示快照不足）: {error}");
    }
}

/// 从一次解析结果里取出可入账的 USD 余额；条件不满足就回 `None`。
///
/// 取**第一条**：与 `balance::row_balance_result` 判低余额告警取同一条，
/// 两边口径必须一致，否则会出现「告警说 4.9、快照记的是另一条的 30」。
fn resolved_usd_balance(usage: &UsageResult) -> Option<f64> {
    if !usage.success {
        return None;
    }
    usage
        .data
        .as_ref()
        .and_then(|items| items.first())
        .filter(|item| item.unit.as_deref() == Some("USD"))
        .and_then(|item| item.remaining)
}

// ============================================================================
// 对账报告：窗口计算（plan §三.3 / §三.4）
// ============================================================================

/// 回看窗口长度：30 天。再早的快照不进报告 —— 中转站价格表会变，
/// 太老的比值没有判别力，只会稀释基线。
const LOOKBACK_SECS: i64 = 30 * 24 * 3600;

/// 返回窗口数上限（新 → 旧）。正常频率（每次余额刷新一枚快照）远够不到，
/// 这是防「高频刷新把 payload 撑爆」的安全阀。基线仍按回看期内全部有效窗口算。
const MAX_WINDOWS: usize = 50;

/// 扣减低于这个数（USD）的窗口不算有效消费 —— 精度噪音，不进比值也不进基线。
const MIN_DEDUCTION_USD: f64 = 0.01;

/// 基线（有效窗口 ratio 的中位数）需要的最少窗口数。不足就不判 `Suspicious`。
const MIN_BASELINE_WINDOWS: usize = 3;

/// `Suspicious` 阈值：`ratio <= baseline × 0.5`。刻意放宽到 2 倍 —— 吸收
/// 「同账号在其他 key/工具上消费」「估算价格表偏差」这类已知噪音，宁可漏报不制造纠纷。
const SUSPICIOUS_RATIO_FRACTION: f64 = 0.5;

/// 一份对账报告（serde camelCase，给前端展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationReport {
    pub relay_id: i64,
    /// 回看期内的快照总数（配成窗口的原料，不是窗口数）。
    pub snapshot_count: usize,
    /// 有效窗口 ratio 的中位数；不足 [`MIN_BASELINE_WINDOWS`] 个有效窗口为 `None`。
    pub baseline_ratio: Option<f64>,
    /// 窗口按新 → 旧排，最多 [`MAX_WINDOWS`] 个。
    pub windows: Vec<ReconciliationWindow>,
}

/// 相邻两枚快照构成的窗口 `[start_secs, end_secs)`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationWindow {
    pub start_secs: i64,
    pub end_secs: i64,
    pub start_balance_usd: f64,
    pub end_balance_usd: f64,
    /// `end - start`：负数 = 扣减，正数 = 充值/返利。
    pub balance_delta_usd: f64,
    /// 窗口内该站名下 provider 的估算成本之和（`proxy_request_logs`，CAST 后的美元数）。
    pub estimated_cost_usd: f64,
    /// `估算 ÷ 扣减`，仅当扣减 > [`MIN_DEDUCTION_USD`] 且估算 > 0 时有值。
    pub ratio: Option<f64>,
    pub flag: WindowFlag,
}

/// 窗口标记。判据是业务事实，由后端定（前端只展示）：
///
/// - [`WindowFlag::SkippedTopUp`]：扣减 <= 0（充值/返利/赠送），不进比值、不进基线。
/// - [`WindowFlag::InsufficientData`]：没有可算的比值（扣减太小或窗口内无估算数据）。
/// - [`WindowFlag::Suspicious`]：`ratio <= baseline × 0.5` 且基线存在 ——
///   实际扣减持续达到预期的 2 倍以上。
/// - [`WindowFlag::Normal`]：其余有比值的窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowFlag {
    Normal,
    SkippedTopUp,
    InsufficientData,
    Suspicious,
}

impl Database {
    /// 算某个中转站的扣费对账报告。
    ///
    /// `provider_keys` 是该站名下全部托管档位的 `(provider_id, app_type)` ——
    /// 归属判据在命令层（`commands::relay::belongs_to_relay`，与 `relay_balance_inputs`
    /// 同一来源），本方法只管按这些 key 聚合 `proxy_request_logs` 的成本。
    pub fn reconciliation_report(
        &self,
        relay_id: i64,
        provider_keys: &[(String, String)],
    ) -> Result<ReconciliationReport, AppError> {
        let since = chrono::Utc::now().timestamp() - LOOKBACK_SECS;
        let snapshots = self.list_balance_snapshots(relay_id, Some(since))?;
        let snapshot_count = snapshots.len();

        // 相邻两枚快照配成窗口（升序 ⇒ 窗口先按旧 → 新算，收尾再反转）。
        let mut windows = Vec::with_capacity(snapshot_count.saturating_sub(1));
        for pair in snapshots.windows(2) {
            let (older, newer) = (&pair[0], &pair[1]);
            let deduction = older.balance_usd - newer.balance_usd; // 正数 = 消费
            let estimated =
                self.estimated_cost_between(provider_keys, older.created_at, newer.created_at)?;
            let ratio =
                (deduction > MIN_DEDUCTION_USD && estimated > 0.0).then(|| estimated / deduction);
            windows.push(ReconciliationWindow {
                start_secs: older.created_at,
                end_secs: newer.created_at,
                start_balance_usd: older.balance_usd,
                end_balance_usd: newer.balance_usd,
                balance_delta_usd: newer.balance_usd - older.balance_usd,
                estimated_cost_usd: estimated,
                ratio,
                flag: WindowFlag::Normal, // 占位；基线算出来后统一分类
            });
        }

        let baseline_ratio = median_baseline(&windows);
        for window in &mut windows {
            window.flag = classify(window, baseline_ratio);
        }
        windows.reverse();
        windows.truncate(MAX_WINDOWS);

        Ok(ReconciliationReport {
            relay_id,
            snapshot_count,
            baseline_ratio,
            windows,
        })
    }

    /// `[start_secs, end_secs)` 内、名下 provider 的估算成本之和（美元）。
    ///
    /// `total_cost_usd` 是 TEXT 美元字符串，必须 CAST（同
    /// `services/usage_stats.rs` 的既有写法）。`(provider_id, app_type)` 成对匹配：
    /// provider id 只在所属 app_type 那一栏下才是这条档位。
    fn estimated_cost_between(
        &self,
        provider_keys: &[(String, String)],
        start_secs: i64,
        end_secs: i64,
    ) -> Result<f64, AppError> {
        if provider_keys.is_empty() {
            return Ok(0.0);
        }

        let mut sql = String::from(
            "SELECT COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0)
             FROM proxy_request_logs
             WHERE created_at >= ?1 AND created_at < ?2
               AND (provider_id, app_type) IN (VALUES ",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(start_secs), Box::new(end_secs)];
        for (i, (provider_id, app_type)) in provider_keys.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str("(?, ?)");
            args.push(Box::new(provider_id.clone()));
            args.push(Box::new(app_type.clone()));
        }
        // 关掉 `IN (VALUES` 那个开括号；每一行 `(?, ?)` 自带收尾。
        sql.push(')');

        let conn = lock_conn!(self.conn);
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        conn.query_row(&sql, params.as_slice(), |row| row.get::<_, f64>(0))
            .map_err(|e| AppError::Database(format!("聚合窗口估算成本失败: {e}")))
    }
}

/// 有效窗口 ratio 的中位数；不足 [`MIN_BASELINE_WINDOWS`] 个有效窗口回 `None`。
fn median_baseline(windows: &[ReconciliationWindow]) -> Option<f64> {
    let mut ratios: Vec<f64> = windows.iter().filter_map(|w| w.ratio).collect();
    if ratios.len() < MIN_BASELINE_WINDOWS {
        return None;
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    let mid = ratios.len() / 2;
    Some(if ratios.len() % 2 == 1 {
        ratios[mid]
    } else {
        (ratios[mid - 1] + ratios[mid]) / 2.0
    })
}

/// 基线定完之后给窗口打标（[`WindowFlag`] 的判据见枚举文档）。
fn classify(window: &ReconciliationWindow, baseline_ratio: Option<f64>) -> WindowFlag {
    let deduction = window.start_balance_usd - window.end_balance_usd;
    if deduction <= 0.0 {
        WindowFlag::SkippedTopUp
    } else if window.ratio.is_none() {
        WindowFlag::InsufficientData
    } else if let Some(baseline) = baseline_ratio {
        let suspicious = window
            .ratio
            .is_some_and(|ratio| ratio <= baseline * SUSPICIOUS_RATIO_FRACTION);
        if suspicious {
            WindowFlag::Suspicious
        } else {
            WindowFlag::Normal
        }
    } else {
        WindowFlag::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Database {
        Database::memory().expect("内存库")
    }

    /// 测试里没法等真实时钟拉开差距，插入后直接改库里的 `created_at`。
    fn set_created_at(db: &Database, id: i64, created_at: i64) {
        let conn = db.conn.lock().expect("拿连接");
        conn.execute(
            "UPDATE relay_balance_snapshots SET created_at = ?1 WHERE id = ?2",
            params![created_at, id],
        )
        .expect("改 created_at");
    }

    #[test]
    fn insert_then_read_roundtrips_every_field() {
        let db = db();
        let id = db
            .insert_balance_snapshot(7, 12.5, "balance_query")
            .expect("插入");

        let rows = db.list_balance_snapshots(7, None).expect("读取");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            BalanceSnapshot {
                id,
                relay_id: 7,
                balance_usd: 12.5,
                source: "balance_query".to_string(),
                created_at: rows[0].created_at, // 时间戳由 DAO 打，见下面单独断言
            }
        );
        // created_at 是「现在」的 Unix 秒 —— 与真实时钟比 ±1 天足够判单位：
        // 若误存成毫秒，这个差会是 1e9 级。
        let now = chrono::Utc::now().timestamp();
        assert!(
            (rows[0].created_at - now).abs() < 86_400,
            "created_at 应是 Unix 秒，实际 {}（now = {now}）",
            rows[0].created_at
        );
    }

    #[test]
    fn snapshots_are_filtered_by_relay() {
        let db = db();
        db.insert_balance_snapshot(1, 10.0, "balance_query")
            .expect("插入");
        db.insert_balance_snapshot(2, 20.0, "balance_query")
            .expect("插入");
        db.insert_balance_snapshot(1, 30.0, "balance_query")
            .expect("插入");

        let rows = db.list_balance_snapshots(1, None).expect("读取");
        assert_eq!(
            rows.iter().map(|r| r.balance_usd).collect::<Vec<_>>(),
            vec![10.0, 30.0],
            "只能看到 relay 1 的快照"
        );
    }

    #[test]
    fn snapshots_are_ordered_by_created_at_ascending() {
        let db = db();
        let a = db
            .insert_balance_snapshot(3, 1.0, "balance_query")
            .expect("插入");
        let b = db
            .insert_balance_snapshot(3, 2.0, "balance_query")
            .expect("插入");
        let c = db
            .insert_balance_snapshot(3, 4.0, "balance_query")
            .expect("插入");
        // 故意让插入顺序与时间顺序相反。
        set_created_at(&db, a, 300);
        set_created_at(&db, b, 100);
        set_created_at(&db, c, 200);

        let rows = db.list_balance_snapshots(3, None).expect("读取");
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b, c, a],
            "必须按 created_at 升序 —— 对账窗口按先后配对快照"
        );
    }

    #[test]
    fn since_secs_keeps_snapshots_at_or_after_the_boundary() {
        let db = db();
        let a = db
            .insert_balance_snapshot(4, 1.0, "balance_query")
            .expect("插入");
        let b = db
            .insert_balance_snapshot(4, 2.0, "balance_query")
            .expect("插入");
        let c = db
            .insert_balance_snapshot(4, 4.0, "balance_query")
            .expect("插入");
        set_created_at(&db, a, 100);
        set_created_at(&db, b, 200);
        set_created_at(&db, c, 300);

        let rows = db.list_balance_snapshots(4, Some(200)).expect("读取");
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b, c],
            "边界值要包含（>=），更早的丢掉"
        );
    }

    #[test]
    fn create_table_is_idempotent() {
        let db = db();
        let conn = db.conn.lock().expect("拿连接");
        create_table(&conn).expect("再建一次不该报错");
    }

    // ------------------------------------------------------------------
    // capture_balance_snapshot：写入条件（plan §三.2）
    // ------------------------------------------------------------------

    fn usage(success: bool, remaining: Option<f64>, unit: Option<&str>) -> UsageResult {
        UsageResult {
            success,
            data: Some(vec![crate::provider::UsageData {
                plan_name: None,
                extra: None,
                is_valid: None,
                invalid_message: None,
                total: None,
                used: None,
                remaining,
                unit: unit.map(str::to_string),
            }]),
            error: None,
        }
    }

    #[test]
    fn capture_writes_one_snapshot_on_success_with_usd_remaining() {
        let db = db();
        capture_balance_snapshot(&db, 7, &usage(true, Some(12.5), Some("USD")));

        let rows = db.list_balance_snapshots(7, None).expect("读取");
        assert_eq!(rows.len(), 1, "成功 + USD 余额必须落一条快照");
        assert_eq!(rows[0].balance_usd, 12.5);
        assert_eq!(rows[0].source, "balance_query");
    }

    #[test]
    fn capture_skips_unsuccessful_resolve() {
        let db = db();
        // 失败路拿到什么 data 都不算数。
        capture_balance_snapshot(&db, 7, &usage(false, Some(12.5), Some("USD")));

        assert_eq!(
            db.list_balance_snapshots(7, None).expect("读取").len(),
            0,
            "success:false 是常态（订阅型分组查不到钱包余额），不能落快照"
        );
    }

    #[test]
    fn capture_skips_when_data_or_remaining_is_missing() {
        let db = db();
        let no_data = UsageResult {
            success: true,
            data: None,
            error: None,
        };
        capture_balance_snapshot(&db, 7, &no_data);
        capture_balance_snapshot(&db, 7, &usage(true, None, Some("USD")));

        assert_eq!(
            db.list_balance_snapshots(7, None).expect("读取").len(),
            0,
            "没有 remaining 就没有可入账的数字"
        );
    }

    #[test]
    fn capture_skips_non_usd_unit() {
        let db = db();
        capture_balance_snapshot(&db, 7, &usage(true, Some(12.5), Some("CNY")));
        capture_balance_snapshot(&db, 7, &usage(true, Some(12.5), None));

        assert_eq!(
            db.list_balance_snapshots(7, None).expect("读取").len(),
            0,
            "快照列名就是 balance_usd，非 USD 的数字进来是错的单位口径"
        );
    }

    #[test]
    fn capture_swallows_insert_failure() {
        let db = db();
        {
            let conn = db.conn.lock().expect("拿连接");
            conn.execute("DROP TABLE relay_balance_snapshots", [])
                .expect("删表制造写入失败");
        }
        // 不该 panic、不该把错误抛出去 —— 对账是旁路能力。
        capture_balance_snapshot(&db, 7, &usage(true, Some(12.5), Some("USD")));
    }

    // ------------------------------------------------------------------
    // reconciliation_report：窗口计算（plan §三.3 / §三.4）
    // ------------------------------------------------------------------

    /// 本站名下那个托管 provider 的 `(provider_id, app_type)`。
    /// id 形状满足 [`crate::relay::managed::is_managed`]：前缀 + 恰好 16 位小写 hex。
    const OWNED_PID: &str = "loongport-0123456789abcdef";

    fn owned_keys() -> Vec<(String, String)> {
        vec![(OWNED_PID.to_string(), "codex".to_string())]
    }

    /// 插一条快照并把 `created_at` 改成指定值（测试没法等真实时钟拉开差距）。
    fn snapshot_at(db: &Database, relay_id: i64, balance_usd: f64, created_at: i64) {
        let id = db
            .insert_balance_snapshot(relay_id, balance_usd, "balance_query")
            .expect("插快照");
        set_created_at(db, id, created_at);
    }

    /// 插一条代理日志。`total_cost_usd` 故意走 TEXT —— 生产列存的是美元字符串，
    /// 聚合必须 CAST 才算得出数。
    fn insert_log(
        db: &Database,
        request_id: &str,
        provider_id: &str,
        app_type: &str,
        created_at: i64,
        total_cost_usd: &str,
    ) {
        let conn = db.conn.lock().expect("拿连接");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, latency_ms, status_code,
                total_cost_usd, created_at
            ) VALUES (?1, ?2, ?3, 'test-model', 100, 200, ?4, ?5)",
            params![
                request_id,
                provider_id,
                app_type,
                total_cost_usd,
                created_at
            ],
        )
        .expect("插日志");
    }

    /// 测试时间基点：几小时前的整点，往前取偏移 —— 30 天回看窗口内的确定性时间轴。
    fn base_time() -> i64 {
        chrono::Utc::now().timestamp() - 10_000
    }

    #[test]
    fn normal_windows_compute_ratio_and_median_baseline() {
        let db = db();
        let base = base_time();
        // 三个窗口，每个扣减 2.0、估算 1.0 ⇒ ratio 全是 0.5，中位数也是 0.5。
        for (offset, balance) in [(100, 10.0), (200, 8.0), (300, 6.0), (400, 4.0)] {
            snapshot_at(&db, 7, balance, base + offset);
        }
        // 两条 TEXT 美元字符串相加（0.60 + 0.40）验证 CAST；t=100 计入、t=400 不计入。
        insert_log(&db, "r0", OWNED_PID, "codex", base + 100, "0.60");
        insert_log(&db, "r1", OWNED_PID, "codex", base + 150, "0.40");
        insert_log(&db, "r2", OWNED_PID, "codex", base + 250, "1.0");
        insert_log(&db, "r3", OWNED_PID, "codex", base + 399, "1.0");
        insert_log(&db, "r4", OWNED_PID, "codex", base + 400, "999"); // 恰好落在窗口右端点之外

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.relay_id, 7);
        assert_eq!(report.snapshot_count, 4);
        assert_eq!(report.windows.len(), 3);
        assert_eq!(report.baseline_ratio, Some(0.5));
        // 新 → 旧
        assert_eq!(report.windows[0].start_secs, base + 300);
        assert_eq!(report.windows[2].start_secs, base + 100);
        for w in &report.windows {
            assert_eq!(w.estimated_cost_usd, 1.0, "估算成本 = {w:?}");
            assert_eq!(w.ratio, Some(0.5));
            assert_eq!(w.flag, WindowFlag::Normal);
            assert_eq!(w.balance_delta_usd, -2.0, "余额变化是负数 = 扣减");
        }
    }

    #[test]
    fn topup_window_is_skipped_and_left_out_of_baseline() {
        let db = db();
        let base = base_time();
        // 中间那个窗口余额上升（充值），其余正常消费。
        for (offset, balance) in [(100, 10.0), (200, 8.0), (300, 12.0), (400, 10.0)] {
            snapshot_at(&db, 7, balance, base + offset);
        }
        insert_log(&db, "r0", OWNED_PID, "codex", base + 150, "1.0");
        insert_log(&db, "r1", OWNED_PID, "codex", base + 350, "1.0");

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        // 有效窗口只有 2 个，基线要 >= 3 个 ⇒ None（充值窗口不进基线）。
        assert_eq!(report.baseline_ratio, None);
        let topup = &report.windows[1]; // 新→旧排完，充值窗口排中间
        assert_eq!((topup.start_secs, topup.end_secs), (base + 200, base + 300));
        assert_eq!(topup.flag, WindowFlag::SkippedTopUp);
        assert_eq!(topup.ratio, None);
        assert_eq!(topup.balance_delta_usd, 4.0, "充值后余额上升为正");
        for w in [&report.windows[0], &report.windows[2]] {
            assert_eq!(w.flag, WindowFlag::Normal);
            assert_eq!(w.ratio, Some(0.5));
        }
    }

    #[test]
    fn tiny_deduction_and_zero_usage_are_insufficient_data() {
        let db = db();
        let base = base_time();
        // [100,200) 扣减 0.005（>0 但 <= 0.01）；[200,300) 无估算数据；[300,400) 正常。
        for (offset, balance) in [(100, 10.0), (200, 9.995), (300, 9.0), (400, 7.0)] {
            snapshot_at(&db, 7, balance, base + offset);
        }
        insert_log(&db, "r0", OWNED_PID, "codex", base + 350, "1.0");

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        // 只有 1 个有效窗口 ⇒ 基线 None。
        assert_eq!(report.baseline_ratio, None);
        let valid = &report.windows[0]; // [300,400)
        assert_eq!(valid.ratio, Some(0.5));
        assert_eq!(
            valid.flag,
            WindowFlag::Normal,
            "没有基线时不判 Suspicious，但仍是有数据的窗口"
        );
        let no_usage = &report.windows[1]; // [200,300)
        assert_eq!(no_usage.estimated_cost_usd, 0.0);
        assert_eq!(no_usage.ratio, None);
        assert_eq!(no_usage.flag, WindowFlag::InsufficientData);
        let tiny = &report.windows[2]; // [100,200)
        assert_eq!(tiny.ratio, None);
        assert_eq!(tiny.flag, WindowFlag::InsufficientData);
    }

    #[test]
    fn half_baseline_ratio_flags_suspicious() {
        let db = db();
        let base = base_time();
        // 5 个窗口：ratio [1.0, 1.0, 1.0, 0.5, 0.3]，中位数 1.0。
        // 0.5 恰好等于 baseline × 0.5 ⇒ 触发（判据是 <=），顺带钉住边界。
        for (offset, balance) in [
            (100, 10.0),
            (200, 9.0),
            (300, 8.0),
            (400, 7.0),
            (500, 6.5),
            (600, 6.2),
        ] {
            snapshot_at(&db, 7, balance, base + offset);
        }
        insert_log(&db, "r0", OWNED_PID, "codex", base + 150, "1.0");
        insert_log(&db, "r1", OWNED_PID, "codex", base + 250, "1.0");
        insert_log(&db, "r2", OWNED_PID, "codex", base + 350, "1.0");
        insert_log(&db, "r3", OWNED_PID, "codex", base + 450, "0.25"); // ratio 0.5
        insert_log(&db, "r4", OWNED_PID, "codex", base + 550, "0.09"); // ratio 0.3

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.baseline_ratio, Some(1.0));
        // 新 → 旧：windows[0] = [500,600)（ratio 0.3），windows[1] = [400,500)（ratio 0.5）。
        assert_eq!(report.windows[0].flag, WindowFlag::Suspicious);
        assert_eq!(report.windows[1].flag, WindowFlag::Suspicious);
        for w in &report.windows[2..] {
            assert_eq!(w.flag, WindowFlag::Normal);
        }
    }

    #[test]
    fn other_providers_and_apps_do_not_leak_into_estimates() {
        let db = db();
        let base = base_time();
        snapshot_at(&db, 7, 10.0, base + 100);
        snapshot_at(&db, 7, 8.0, base + 200);
        // 另一个中转站的托管 provider、同 id 挂在别的 app_type 下，都不许混进来。
        insert_log(&db, "mine", OWNED_PID, "codex", base + 150, "0.50");
        insert_log(
            &db,
            "foreign",
            "loongport-fedcba9876543210",
            "codex",
            base + 150,
            "999",
        );
        insert_log(&db, "wrong-app", OWNED_PID, "claude", base + 150, "888");
        // 别的 relay 的快照也不该配进本站的窗口。
        snapshot_at(&db, 8, 100.0, base + 150);

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.windows.len(), 1, "别的 relay 的快照不该产生窗口");
        let w = &report.windows[0];
        assert_eq!(w.estimated_cost_usd, 0.50, "估算成本 = {w:?}");
        assert_eq!(w.ratio, Some(0.25));
    }

    #[test]
    fn snapshots_older_than_30_days_are_ignored() {
        let db = db();
        let now = chrono::Utc::now().timestamp();
        snapshot_at(&db, 7, 100.0, now - 40 * 86_400);
        snapshot_at(&db, 7, 90.0, now - 35 * 86_400);
        snapshot_at(&db, 7, 10.0, now - 5 * 86_400);
        snapshot_at(&db, 7, 8.0, now - 86_400);

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.snapshot_count, 2, "30 天窗口外的快照不进报告");
        assert_eq!(report.windows.len(), 1);
        assert_eq!(report.windows[0].start_secs, now - 5 * 86_400);
    }

    #[test]
    fn windows_are_capped_at_50_newest_first() {
        let db = db();
        let now = chrono::Utc::now().timestamp();
        // 60 枚快照（每小时一枚）⇒ 59 个窗口，只回最近 50 个。
        for i in 0..60 {
            snapshot_at(&db, 7, 100.0 - i as f64, now - (60 - i) * 3600);
        }

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.snapshot_count, 60);
        assert_eq!(report.windows.len(), 50);
        assert_eq!(report.windows[0].end_secs, now - 3600, "最新的窗口排最前");
    }

    #[test]
    fn report_without_enough_snapshots_is_empty() {
        let db = db();
        snapshot_at(&db, 7, 10.0, base_time());

        let report = db.reconciliation_report(7, &owned_keys()).expect("算报告");
        assert_eq!(report.snapshot_count, 1);
        assert!(report.windows.is_empty(), "快照不足两枚 ⇒ 没有窗口");
        assert_eq!(report.baseline_ratio, None);
    }

    /// 老库（v13，没有本表）升级后必须有这张表且可用。
    #[test]
    fn an_upgraded_database_gets_the_table() {
        let conn = Connection::open_in_memory().expect("内存库");
        // 造一个停在 v13 的库：版本表写 13，不建快照表。
        conn.execute(
            "CREATE TABLE loongport_schema_version (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                version INTEGER NOT NULL
            )",
            [],
        )
        .expect("建版本表");
        conn.execute(
            "INSERT INTO loongport_schema_version (id, version) VALUES (1, 13)",
            [],
        )
        .expect("设为 v13");
        assert!(
            !crate::Database::table_exists(&conn, "relay_balance_snapshots").expect("查表"),
            "前提：升级前本表不存在（否则这条闸没有判别力）"
        );

        crate::database::loongport_schema::apply(&conn).expect("迁移");

        assert!(
            crate::Database::table_exists(&conn, "relay_balance_snapshots").expect("查表"),
            "v13 → v14 必须建出快照表"
        );
        conn.execute(
            "INSERT INTO relay_balance_snapshots (relay_id, balance_usd, source, created_at)
             VALUES (7, 12.5, 'balance_query', 1000)",
            [],
        )
        .expect("迁移后必须可写");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM relay_balance_snapshots", [], |r| {
                r.get(0)
            })
            .expect("迁移后必须可读");
        assert_eq!(n, 1);
        let version: i32 = conn
            .query_row(
                "SELECT version FROM loongport_schema_version WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("读版本");
        assert_eq!(
            version,
            crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
        );
    }
}
