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
